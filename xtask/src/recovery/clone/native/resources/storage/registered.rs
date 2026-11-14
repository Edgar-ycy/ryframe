use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{
    super::{
        super::{json as json_io, lifecycle::identity},
        common,
    },
    model, process,
};
use crate::{
    Result,
    cli::CloneSide,
    recovery::clone::native::lifecycle::{lock, state},
};

pub(crate) fn stage(side: CloneSide) -> String {
    format!("storage-{}", side.as_str())
}

pub(crate) fn records(run_dir: &Path, side: CloneSide, verify_results: bool) -> Result<Vec<Value>> {
    let value = state::load(run_dir, verify_results)?;
    Ok(state::attempts(&value)?
        .iter()
        .filter(|attempt| {
            attempt.get("stage").and_then(Value::as_str) == Some(stage(side).as_str())
        })
        .cloned()
        .collect())
}

pub(crate) fn controller_binding(run_dir: &Path, attempt: &Value) -> Result<Value> {
    let number = attempt
        .get("number")
        .and_then(Value::as_u64)
        .ok_or("存储阶段序号无效")?;
    let (binding, owner) = state::controller_record(run_dir, number, attempt)?;
    identity::validate(&owner, run_dir)?;
    if owner.get("manifest_sha256")
        != Some(&json!(
            super::super::super::digest::file(&run_dir.join("manifest.json"))?.sha256
        ))
    {
        return Err("存储控制器未绑定当前复制清单".into());
    }
    Ok(binding)
}

pub(crate) fn active(
    backend: &Path,
    run_dir: &Path,
    mode: &str,
    number: u64,
    side: CloneSide,
) -> Result<Value> {
    let value = state::load(run_dir, !matches!(mode, "stop" | "recover"))?;
    let attempt = state::attempts(&value)?
        .last()
        .ok_or("存储控制必须通过统一阶段入口执行")?;
    if attempt.get("number").and_then(Value::as_u64) != Some(number)
        || attempt.get("stage").and_then(Value::as_str) != Some(stage(side).as_str())
        || attempt.get("mode").and_then(Value::as_str) != Some(mode)
        || attempt.get("status").and_then(Value::as_str) != Some("running")
    {
        return Err("存储控制不是当前唯一已登记阶段".into());
    }
    let binding = controller_binding(run_dir, attempt)?;
    let controller = common::read_bound(backend, &binding, "存储控制器")?;
    let owner = controller.get("owner").ok_or("存储控制器缺少 owner")?;
    if owner != &identity::current()?
        || json_io::read(&run_dir.join("run.lock/owner.json"))? != *owner
    {
        return Err("存储动作必须由原登记控制器持有统一运行锁".into());
    }
    let _ = lock::owner_observation(run_dir)?;
    Ok(binding)
}

pub(crate) fn root(backend: &Path, run_dir: &Path, side: CloneSide) -> Result<PathBuf> {
    let path = run_dir.join(stage(side));
    common::ensure_directory(backend, &path, "存储登记目录")?;
    Ok(path)
}

pub(crate) fn registration(
    backend: &Path,
    run_dir: &Path,
    side: CloneSide,
) -> Result<(Value, Value)> {
    let root = common::local_directory(backend, &run_dir.join(stage(side)), "存储登记目录")?;
    let saved = json_io::read(&root.join("registration.json"))?;
    let saved = json_io::exact(&saved, &["request", "manifest", "side"], "存储登记")?;
    let request_path = common::bound_file(
        backend,
        saved.get("request").ok_or("存储登记缺少请求")?,
        "存储固定请求",
    )?;
    if saved.get("manifest") != Some(&common::binding(&run_dir.join("manifest.json"))?)
        || saved.get("side").and_then(Value::as_str) != Some(side.as_str())
        || request_path != root.join("request.json")
    {
        return Err("存储登记不属于固定运行和明确侧".into());
    }
    let descriptor = saved.get("request").unwrap().clone();
    let request = json_io::read(&request_path)?;
    if request.get("manifest") != saved.get("manifest")
        || request.get("side").and_then(Value::as_str) != Some(side.as_str())
    {
        return Err("存储请求与登记不符".into());
    }
    Ok((request, descriptor))
}

pub(crate) fn register(
    backend: &Path,
    run_dir: &Path,
    manifest: &Value,
    side: CloneSide,
    filename: Option<&Path>,
) -> Result<(Value, Value, model::Validated)> {
    let root = root(backend, run_dir, side)?;
    let registration_path = root.join("registration.json");
    let (request, descriptor) = if registration_path.exists() {
        let (request, descriptor) = registration(backend, run_dir, side)?;
        if let Some(filename) = filename {
            let supplied = json_io::read(&common::common_file(backend, filename, "存储重启请求")?)?;
            if supplied != request {
                return Err("不能替换同一存储运行的固定请求".into());
            }
        }
        (request, descriptor)
    } else {
        let filename = filename.ok_or("首次存储重启必须显式提供请求")?;
        let source = common::common_file(backend, filename, "存储重启请求")?;
        let request = json_io::read(&source)?;
        model::validate(backend, run_dir, manifest, side, &request)?;
        common::write_new(&root.join("request.json"), &request)?;
        let descriptor = common::binding(&root.join("request.json"))?;
        common::write_new(
            &registration_path,
            &json!({
                "request": descriptor,
                "manifest": common::binding(&run_dir.join("manifest.json"))?,
                "side": side.as_str(),
            }),
        )?;
        (request, descriptor)
    };
    let validated = model::validate(backend, run_dir, manifest, side, &request)?;
    Ok((request, descriptor, validated))
}

pub(crate) fn previous_attempts(
    backend: &Path,
    run_dir: &Path,
    side: CloneSide,
    descriptor: &Value,
    before: Option<u64>,
) -> Result<Vec<(Value, Value)>> {
    let mut result = Vec::new();
    for attempt in records(run_dir, side, false)? {
        let number = attempt["number"].as_u64().unwrap();
        if attempt.get("mode").and_then(Value::as_str) != Some("restart")
            || before.is_some_and(|value| number >= value)
        {
            continue;
        }
        let output = run_dir.join(stage(side)).join(format!("a{number:04}"));
        let controller = controller_binding(run_dir, &attempt)?;
        if let Some(observed) =
            process::inspect_attempt(backend, &output, descriptor, &controller, number)?
        {
            result.push((attempt, observed));
        }
    }
    Ok(result)
}

pub(crate) fn require_previous_stopped(
    backend: &Path,
    run_dir: &Path,
    request: &Value,
    descriptor: &Value,
    before: Option<u64>,
) -> Result<()> {
    common::require_recorded_stopped(&request["previous"]["identity"])?;
    for (_, observed) in previous_attempts(
        backend,
        run_dir,
        side_from_request(request)?,
        descriptor,
        before,
    )? {
        if observed.get("state").and_then(Value::as_str) == Some("recorded") {
            common::require_recorded_stopped(&observed["identity"])?;
        }
    }
    for key in ["api_url", "console_url"] {
        common::require_closed_port(
            request
                .get(key)
                .and_then(Value::as_str)
                .ok_or("存储请求缺少监听地址")?,
        )?;
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn registered(
    backend: &Path,
    run_dir: &Path,
    side: CloneSide,
    before: Option<u64>,
) -> Result<Option<Value>> {
    common::local_directory(backend, run_dir, "复制运行目录")?;
    let root = run_dir.join(stage(side));
    let selected = records(run_dir, side, true)?
        .into_iter()
        .filter(|attempt| before.is_none_or(|value| attempt["number"].as_u64().unwrap() < value))
        .collect::<Vec<_>>();
    if !root.exists() && selected.is_empty() {
        return Ok(None);
    }
    let (request, descriptor) = registration(backend, run_dir, side)?;
    let changes = selected
        .iter()
        .filter(|attempt| {
            matches!(
                attempt.get("mode").and_then(Value::as_str),
                Some("restart" | "stop" | "recover")
            )
        })
        .collect::<Vec<_>>();
    let attempt = changes
        .last()
        .ok_or("存储最新改变状态的阶段不是已发布成功重启")?;
    if attempt.get("mode").and_then(Value::as_str) != Some("restart")
        || attempt.get("status").and_then(Value::as_str) != Some("passed")
    {
        return Err("存储最新改变状态的阶段不是已发布成功重启".into());
    }
    let number = attempt["number"].as_u64().unwrap();
    if selected.iter().any(|item| {
        item["number"].as_u64().unwrap() > number
            && item.get("status").and_then(Value::as_str) != Some("passed")
    }) {
        return Err("存储有后续失败或未完成观察，不能回退旧成功".into());
    }
    let manifest = json_io::read(&run_dir.join("manifest.json"))?;
    model::validate(backend, run_dir, &manifest, side, &request)?;
    let output = root.join(format!("a{number:04}"));
    let controller = controller_binding(run_dir, attempt)?;
    let observed = process::inspect_attempt(backend, &output, &descriptor, &controller, number)?
        .ok_or("成功存储阶段缺少完整实际进程和启动收据")?;
    if observed.get("state").and_then(Value::as_str) != Some("recorded")
        || observed.get("launch_receipt").is_none_or(Value::is_null)
    {
        return Err("成功存储阶段缺少完整实际进程和启动收据".into());
    }
    let storage = json!({
        "identity": observed["identity"],
        "sha256": request["executable"]["sha256"],
        "process_receipt": observed["process_receipt"],
        "launch_receipt": observed["launch_receipt"],
    });
    let ready = json_io::read(&output.join("ready.json"))?;
    if ready
        != json!({
            "storage": storage,
            "request": descriptor,
            "controller": controller,
            "attempt": number,
            "listeners": [request["api_url"], request["console_url"]],
        })
    {
        return Err("存储就绪证据与实际启动收据不符".into());
    }
    let result = common::read_bound(
        backend,
        attempt.get("result").ok_or("存储成功阶段缺少结果")?,
        "存储阶段结果",
    )?;
    if let Some(recorded) = result.get("fixture_handoff") {
        let handoff = super::transferred(backend, run_dir, &manifest, side, &request, false)?;
        if handoff.as_ref().map(|handoff| &handoff.binding) != Some(recorded) {
            return Err("存储重启结果未绑定原 RustFS 移交收据".into());
        }
    }
    let expected = json!({
        "status": "storage_restarted",
        "side": side.as_str(),
        "request": descriptor,
        "storage": storage,
        "ready": common::binding(&output.join("ready.json"))?,
        "attempt": number,
        "data_directory": request["data_directory"],
        "api_url": request["api_url"],
        "console_url": request["console_url"],
    });
    for (key, value) in expected.as_object().unwrap() {
        if result.get(key) != Some(value) {
            return Err("存储启动尚未取得同一外层发布结果".into());
        }
    }
    Ok(Some(json!({
        "request": descriptor,
        "storage": storage,
        "attempt": number,
        "data_directory": request["data_directory"],
        "api_url": request["api_url"],
        "console_url": request["console_url"],
        "restart_result": attempt["result"],
    })))
}

#[allow(dead_code)]
pub(crate) fn current(backend: &Path, run_dir: &Path, side: CloneSide) -> Result<Option<Value>> {
    let Some(result) = registered(backend, run_dir, side, None)? else {
        return Ok(None);
    };
    let request = common::read_bound(backend, &result["request"], "存储固定请求")?;
    if !process::running(&result["storage"]["identity"], &request, true)? {
        return Err("已登记存储进程已退出，必须显式同目录重启".into());
    }
    Ok(Some(result))
}

pub(crate) fn observe_or_stop(
    backend: &Path,
    run_dir: &Path,
    request: &Value,
    descriptor: &Value,
    output: &Path,
    mode: &str,
    number: u64,
) -> Result<Value> {
    let previous = previous_attempts(
        backend,
        run_dir,
        side_from_request(request)?,
        descriptor,
        Some(number),
    )?;
    if previous.is_empty() {
        return Err("不存在本统一目录实际启动的存储；不能回收原始服务".into());
    }
    if matches!(mode, "stop" | "recover") {
        for (_, observed) in &previous {
            if observed.get("state").and_then(Value::as_str) == Some("recorded") {
                process::running(&observed["identity"], request, false)?;
            }
        }
    }
    let mut results = Vec::new();
    for (attempt, observed) in previous {
        let number = attempt["number"].as_u64().unwrap();
        if observed.get("state").and_then(Value::as_str) == Some("not_started") {
            results.push(json!({
                "attempt": number,
                "state": "stopped",
                "identity": null,
            }));
            continue;
        }
        if matches!(mode, "stop" | "recover") {
            let item_output = output.join(format!("p{number:04}"));
            common::ensure_new_directory(backend, &item_output, "存储回收证据目录")?;
            let result = process::stop_record(request, &observed, &item_output)?;
            results.push(json!({"attempt": number, "state": result["state"], "identity": result["identity"], "terminated": result["terminated"], "process_receipt": result["process_receipt"]}));
        } else {
            let alive = process::running(&observed["identity"], request, true)?;
            results.push(json!({
                "attempt": number,
                "state": if alive { "running" } else { "stopped" },
                "identity": observed["identity"],
                "process_receipt": observed["process_receipt"],
            }));
        }
    }
    if matches!(mode, "stop" | "recover") {
        for key in ["api_url", "console_url"] {
            common::require_closed_port(request[key].as_str().unwrap())?;
        }
    }
    Ok(json!({
        "status": if mode == "recover" { "storage_recovered" } else if mode == "stop" { "storage_stopped" } else { "storage_status" },
        "side": request["side"],
        "request": descriptor,
        "processes": results,
        "reconciliation_required": true,
        "object_api_calls": 0,
        "database_calls": 0,
        "resources_deleted": false,
    }))
}

fn side_from_request(request: &Value) -> Result<CloneSide> {
    match request.get("side").and_then(Value::as_str) {
        Some("source") => Ok(CloneSide::Source),
        Some("target") => Ok(CloneSide::Target),
        _ => Err("存储请求 side 无效".into()),
    }
}

pub(crate) fn unregistered(side: CloneSide) -> Value {
    json!({
        "status": "storage_unregistered",
        "side": side.as_str(),
        "copy_usable": false,
        "processes": [],
    })
}
