//! 复用完整生产者状态机、角色收据解码和监督树权威；从不发送start/stop。
use crate::{
    Result,
    recovery::{
        clone::native::{
            digest, json as document, lifecycle::state, resources::common, seed,
            target_verify::guard::Inputs,
        },
        full_stack::worker::{self, role::Role},
    },
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

pub(super) fn capture(
    backend: &Path,
    product: &Path,
    run: &Path,
    saved: &Value,
    previous: &seed::history::Post,
) -> Result<Value> {
    let (prepared, failed, stopped) = stages(saved)?;
    let prepared_value = common::read_bound(backend, &prepared["result"], "原prepare结果")?;
    let handoff = common::binding(&previous.runtime.join("handoff.json"))?;
    let handoff_value = common::read_bound(backend, &handoff, "原handoff")?;
    if prepared_value["status"] != "post_copy_api_prepared"
        || prepared_value["handoff"] != handoff
        || handoff_value["registration"] != previous.descriptor
        || handoff_value["worker_started"] != false
    {
        return Err("旧prepare不是活动登记的API-only交接".into());
    }
    let runtime = crate::recovery::clone::native::cache::handoff::registration::cleanup_runtime(
        backend, run, previous,
    )?;
    let stopped_value = common::read_bound(backend, &stopped["result"], "原停止结果")?;
    stopped_result(backend, previous, &runtime, &stopped_value)?;
    let (controller, owner) = historical_controller(run, failed)?;
    let (_, stop_owner) = historical_controller(run, stopped)?;
    let number = failed["number"].as_u64().ok_or("失败启动序号无效")?;
    let failure = common::binding(&run.join(format!("failure-{number:04}.json")))?;
    let diagnosis = common::read_bound(backend, &failure, "原失败诊断")?;
    if diagnosis["format_version"] != 2
        || diagnosis["kind"] != "devex-stage-failure"
        || diagnosis["attempt"] != failed["number"]
        || diagnosis["stage"] != "runtime-target"
        || diagnosis["mode"] != "start"
        || diagnosis["error_type"] != failed["error_type"]
        || diagnosis["controller"] != controller
        || diagnosis["operation_id"] != "clone.runtime-target.start"
    {
        return Err("原失败诊断不属于最新原生启动控制器".into());
    }
    let history = producer(
        backend,
        product,
        previous,
        &runtime,
        &handoff_value["runtime"],
        &owner,
    )?;
    let api = common::binding(&previous.runtime.join("api.json"))?;
    stop_archive(
        backend,
        &previous.runtime,
        &runtime,
        &stop_owner["identity"],
    )?;
    closed(backend, previous, &handoff_value, &runtime)?;
    let result = json!({"prepare":prepared["result"],"handoff":handoff,"runtime":common::binding(&previous.runtime.join("runtime.json"))?,
        "binaries":common::binding(&previous.runtime.join("binaries.json"))?,"api_process":api,"producer_history":history["history"],
        "failed_start":{"attempt":failed["number"],"controller":controller,"failure":failure},
        "stopped":{"attempt":stopped["number"],"result":stopped["result"]}});
    let mut paths = BTreeSet::new();
    super::super::files::bindings(backend, &result, &mut paths)?;
    let protected = Inputs::open(paths)?;
    if state::load(run, true)? != *saved {
        return Err("原运行历史在关闭证明采集期间变化".into());
    }
    closed(backend, previous, &handoff_value, &runtime)?;
    protected.check()?;
    Ok(result)
}

fn stopped_result(
    backend: &Path,
    previous: &seed::history::Post,
    runtime: &Value,
    value: &Value,
) -> Result<()> {
    let processes = json!({"api":{"state":"stopped","identity":null,"ready":false}});
    if value["format_version"] != 1
        || value["kind"] != "devex-clone-runtime-control"
        || value["operation"] != "stop"
        || value["scope_id"] != runtime["scope_id"]
        || value["processes"] != processes
        || !super::super::files::same_directory(
            backend,
            &value["runtime_directory"],
            &previous.runtime,
        )?
        || !super::super::files::same_file(
            backend,
            &value["producer_history"],
            &previous.runtime.join("producer-history.json"),
        )?
    {
        return Err("原停止结果不是同一完整API关闭证明".into());
    }
    Ok(())
}

fn historical_controller(run: &Path, row: &Value) -> Result<(Value, Value)> {
    let mut original = row.clone();
    original["status"] = json!("running");
    for field in ["finished_at", "result", "error_type"] {
        original[field] = Value::Null;
    }
    state::controller_record(
        run,
        row["number"].as_u64().ok_or("历史控制器序号无效")?,
        &original,
    )
}

fn closed(
    backend: &Path,
    previous: &seed::history::Post,
    handoff: &Value,
    runtime: &Value,
) -> Result<()> {
    for name in ["worker-control.lock", "api-control.lock"] {
        super::super::files::absent(&previous.runtime.join(name))?;
    }
    super::super::files::absent(&previous.runtime.join("worker.json"))?;
    for role in [Role::Api, Role::Worker] {
        if worker::controller::observe_bound(
            role,
            &super::super::runtime::product_root(backend, runtime)?,
            &previous.runtime,
            &previous.request["api_environment"],
        )?["state"]
            != "stopped"
        {
            return Err("旧API或Worker完整监督树尚未停止，保留原失败".into());
        }
    }
    common::require_closed_port(document::text(&handoff["api_url"], "原API地址")?)?;
    common::require_closed_port(document::text(
        &runtime["worker_ready_url"],
        "原Worker地址",
    )?)
}

fn gone(identity: &Value) -> Result<()> {
    let identity = common::validate_process_identity(identity)?;
    let pid = u32::try_from(identity["pid"].as_u64().ok_or("原生产者PID无效")?)?;
    match common::process_identity(pid)? {
        None => Ok(()),
        Some(current)
            if current["pid"] == identity["pid"]
                && current["started"]
                    .as_str()
                    .and_then(|v| v.parse::<u128>().ok())
                    .zip(
                        identity["started"]
                            .as_str()
                            .and_then(|v| v.parse::<u128>().ok()),
                    )
                    .is_some_and(|(new, old)| new > old) =>
        {
            Ok(())
        }
        _ => Err("原生产者或监督器仍运行或身份未知，不能修订".into()),
    }
}

fn archive(backend: &Path, runtime: &Path, contract: &Value, token: &str) -> Result<Value> {
    if token.len() != 32
        || !token
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
    {
        return Err("原生操作ID无效".into());
    }
    let path = common::local_directory(
        backend,
        &runtime.join("api-control-receipts").join(token),
        "原生角色归档",
    )?;
    let mut paths = Vec::new();
    let mut names = BTreeSet::new();
    for entry in std::fs::read_dir(&path)? {
        let path = entry?.path();
        let name = path
            .file_stem()
            .and_then(|v| v.to_str())
            .ok_or("角色归档文件名无效")?
            .to_owned();
        if path.extension().and_then(|v| v.to_str()) != Some("json")
            || ![
                "owner",
                "request",
                "candidate",
                "supervisor",
                "progress",
                "result",
                "preparation-failed",
            ]
            .contains(&name.as_str())
            || !names.insert(name)
        {
            return Err("角色归档包含未知或重复文件，保留现场".into());
        }
        paths.push(common::common_file(backend, &path, "原生角色归档文件")?);
    }
    let protected = Inputs::open(paths.iter().cloned())?;
    let mut record = json!({});
    for path in &paths {
        let name = path
            .file_stem()
            .and_then(|v| v.to_str())
            .ok_or("归档名称无效")?;
        let binding = common::binding(path)?;
        record[name] = common::read_bound(backend, &binding, "原生角色归档文件")?;
    }
    if let Some(binding) = record["request"].get("environment_binding") {
        let path = common::bound_file(backend, &binding["file"], "原生启动冻结环境")?;
        let metadata = std::fs::symlink_metadata(&path)?;
        if json!(common::device_inode_for(&path, &metadata)?) != binding["identity"] {
            return Err("原生归档冻结环境文件身份变化，保留原收据".into());
        }
    }
    let runtime_binding = common::binding(&runtime.join("runtime.json"))?;
    let evidence = runtime.join("runtime-evidence.json");
    let source = digest::plan_hash(
        &json!({"runtime":{"bytes":runtime_binding["bytes"],"sha256":runtime_binding["sha256"]},
        "runtime_evidence":if evidence.try_exists()? { let v=common::binding(&evidence)?; json!({"bytes":v["bytes"],"sha256":v["sha256"]}) } else {Value::Null}}),
    )?;
    validate_archive(
        backend,
        runtime,
        contract,
        token,
        &runtime_binding,
        &source,
        &record,
    )?;
    protected.check()?;
    Ok(record)
}

fn stop_archive(
    backend: &Path,
    runtime: &Path,
    contract: &Value,
    controller: &Value,
) -> Result<()> {
    let directory = common::local_directory(
        backend,
        &runtime.join("api-control-receipts"),
        "原角色归档根",
    )?;
    let mut matches = 0;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let owner = common::read_bound(
            backend,
            &common::binding(&path.join("owner.json"))?,
            "原角色owner",
        )?;
        if owner["identity"] != *controller {
            continue;
        }
        let token = entry
            .file_name()
            .into_string()
            .map_err(|_| "角色归档名称无效")?;
        let record = archive(backend, runtime, contract, &token)?;
        if record["request"]["operation"] != "stop"
            || record["result"]["outcome"] != "succeeded"
            || record["result"]["state"] != "stopped"
        {
            return Err("原stop控制器没有唯一成功的原生完整关闭结果".into());
        }
        matches += 1;
    }
    if matches != 1 {
        return Err("原stop控制器与角色关闭归档不唯一或缺失".into());
    }
    Ok(())
}

fn producer(
    backend: &Path,
    product: &Path,
    previous: &seed::history::Post,
    runtime: &Value,
    runtime_binding: &Value,
    owner: &Value,
) -> Result<Value> {
    let history = seed::source_registration::lineage::runtime::collect_bound(
        backend,
        product,
        &previous.runtime,
        runtime_binding,
    )?;
    let generations = document::array(&history["generations"], "完整原生生产者代次")?;
    if generations.len() != 1 {
        return Err("旧API准备仅接受唯一实际启动代次；完整历史存在其他生产者，保留现场".into());
    }
    let latest = generations
        .last()
        .filter(|row| row["role"] == "api")
        .ok_or("缺少原启动的API创建身份，不能把未知启动标为失败关闭")?;
    let api = common::binding(&previous.runtime.join("api.json"))?;
    if common::read_bound(backend, &api, "原API角色")?["identity"] != latest["identity"] {
        return Err("原API收据不同于最新生产者代次".into());
    }
    let events = common::read_bound(backend, &history["history"], "完整原生产者历史")?;
    let last = document::array(&events["events"], "完整历史事件")?
        .last()
        .ok_or("原生产者历史为空")?;
    if last["event"] != "stopped" || last["role"] != "api" || last["identity"] != latest["identity"]
    {
        return Err("完整原生历史未发布同一API代次的关闭事件，保留未知现场".into());
    }
    for generation in generations {
        if generation["role"] != "api" {
            return Err("API-only旧运行出现Worker生产者代次".into());
        }
        let token = document::text(&generation["token"], "原生启动操作ID")?;
        let archived = archive(backend, &previous.runtime, runtime, token)?;
        if archived["request"]["operation"] != "start"
            || archived["result"]["identity"] != generation["identity"]
        {
            return Err("完整历史没有同一操作的原生角色结果".into());
        }
        if generation == latest
            && (archived["owner"]["identity"] != owner["identity"]
                || archived["request"]["environment_binding"]["file"]
                    != previous.request["api_environment"])
        {
            return Err("最新原生启动不是原失败Stage控制器和原冻结环境".into());
        }
        for identity in [&generation["identity"], &archived["candidate"]["identity"]] {
            gone(identity)?;
        }
    }
    let tree = common::read_bound(
        backend,
        &common::binding(&previous.runtime.join("api-tree.json"))?,
        "原完整监督树",
    )?;
    if tree["operation_id"] != latest["token"] || tree["process"] != latest["identity"] {
        return Err("原完整监督树不是唯一失败启动代次".into());
    }
    Ok(history)
}

fn stages(saved: &Value) -> Result<(&Value, &Value, &Value)> {
    let attempts = state::attempts(saved)?;
    let prepared = attempts
        .iter()
        .rfind(|row| {
            row["stage"] == "post-copy" && row["mode"] == "prepare" && row["status"] == "passed"
        })
        .ok_or("环境修订必须绑定已发布的旧prepare")?;
    let failed = attempts
        .iter()
        .rfind(|row| {
            row["stage"] == "runtime-target"
                && row["mode"] == "start"
                && row["number"].as_u64() > prepared["number"].as_u64()
        })
        .filter(|row| row["status"] == "failed")
        .ok_or("环境修订仅接受最新已明确失败的API启动")?;
    let stopped = attempts
        .iter()
        .rfind(|row| row["stage"] == "runtime-target")
        .filter(|row| {
            row["mode"] == "stop"
                && row["status"] == "passed"
                && row["number"].as_u64() > failed["number"].as_u64()
        })
        .ok_or("失败API必须由最新精确stop收尾，未知或后续操作不能采纳")?;
    Ok((prepared, failed, stopped))
}

fn validate_archive(
    backend: &Path,
    runtime: &Path,
    contract: &Value,
    token: &str,
    runtime_binding: &Value,
    source: &str,
    record: &Value,
) -> Result<()> {
    if record["owner"]["operation_id"] != token {
        return Err("原生归档目录与操作ID不同".into());
    }
    for mode in ["request", "candidate", "supervisor", "progress", "result"] {
        if ["candidate", "supervisor", "progress"].contains(&mode)
            && record["request"]["operation"] == "stop"
        {
            continue;
        }
        worker::execute(&serde_json::to_vec(
            &json!({"format_version":1,"kind":"ryframe-full-stack-worker","operation":"validate",
            "arguments":{"mode":mode,"directory":runtime,"source":source,"owner":record["owner"],"request":record["request"],
                "value":record[mode],"candidate":record["candidate"],"supervisor":record["supervisor"],
                "runtime_sha256":runtime_binding["sha256"],"backend":super::super::runtime::product_root(backend, contract)?,"runtime":contract}}),
        )?)?;
    }
    if record["request"]["operation"] != "start"
        && ["candidate", "supervisor", "progress"]
            .iter()
            .any(|field| !record[*field].is_null())
    {
        return Err("非start原生归档包含监督收据，保留现场".into());
    }
    if record["request"]["operation"] == "start"
        && record["result"]["outcome"] == "succeeded"
        && (record["progress"]["phase"] != "ready"
            || record["progress"]["identity"] != record["result"]["identity"])
    {
        return Err("成功原生启动归档缺少同一身份的ready进度".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_evidence_accepts_same_managed_positions_without_rewriting_and_rejects_other_locations()
    {
        let backend = crate::workspace::root_dir();
        std::fs::create_dir_all(backend.join(".local-tests")).unwrap();
        let temporary = tempfile::Builder::new()
            .prefix("post-stop-path-")
            .tempdir_in(backend.join(".local-tests"))
            .unwrap();
        let root = temporary.path().join("中文 交接");
        let directory = root.join("post-copy").join("runtime");
        let other = root.join("other");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::create_dir(&other).unwrap();
        let history = directory.join("producer-history.json");
        std::fs::write(&history, b"{\"original\":true}\n").unwrap();
        let copy = other.join("producer-history.json");
        std::fs::write(&copy, std::fs::read(&history).unwrap()).unwrap();
        let post = seed::history::Post {
            request: Value::Null,
            descriptor: Value::Null,
            runtime: directory.clone(),
        };
        let runtime = json!({"scope_id":"post-path-tests"});
        let original = json!({"format_version":1,"kind":"devex-clone-runtime-control","operation":"stop",
            "runtime_directory":directory,"scope_id":"post-path-tests",
            "processes":{"api":{"state":"stopped","identity":null,"ready":false}},"producer_history":history});
        let history_before = common::binding(&history).unwrap();
        for (directory, history) in [
            (
                root.join("post-copy/runtime"),
                root.join("post-copy/runtime/producer-history.json"),
            ),
            (
                std::fs::canonicalize(&directory).unwrap(),
                std::fs::canonicalize(&history).unwrap(),
            ),
        ] {
            let mut value = original.clone();
            value["runtime_directory"] = json!(directory);
            value["producer_history"] = json!(history);
            let before = serde_json::to_vec(&value).unwrap();
            stopped_result(&backend, &post, &runtime, &value).unwrap();
            assert_eq!(serde_json::to_vec(&value).unwrap(), before);
            assert_eq!(
                common::binding(&post.runtime.join("producer-history.json")).unwrap(),
                history_before
            );
        }
        let mut wrong = original.clone();
        wrong["runtime_directory"] = json!(other);
        assert!(
            stopped_result(&backend, &post, &runtime, &wrong)
                .unwrap_err()
                .to_string()
                .contains("同一完整API关闭证明")
        );
        wrong = original;
        wrong["producer_history"] = json!(copy);
        assert!(
            stopped_result(&backend, &post, &runtime, &wrong)
                .unwrap_err()
                .to_string()
                .contains("同一完整API关闭证明")
        );
        assert_eq!(
            common::binding(&post.runtime.join("producer-history.json")).unwrap(),
            history_before
        );
    }

    #[test]
    fn amendment_requires_latest_failed_start_and_latest_exact_stop_without_old_success_fallback() {
        let original = json!({"attempts":[
            {"number":1,"stage":"post-copy","mode":"prepare","status":"passed"},
            {"number":2,"stage":"runtime-target","mode":"start","status":"failed"},
            {"number":3,"stage":"runtime-target","mode":"stop","status":"passed"}]});
        let (prepare, failed, stopped) = stages(&original).unwrap();
        assert_eq!(
            (
                prepare["number"].as_u64(),
                failed["number"].as_u64(),
                stopped["number"].as_u64()
            ),
            (Some(1), Some(2), Some(3))
        );
        let mut later_success = original.clone();
        later_success["attempts"]
            .as_array_mut()
            .unwrap()
            .push(json!({"number":4,"stage":"runtime-target","mode":"start","status":"passed"}));
        assert!(
            stages(&later_success)
                .unwrap_err()
                .to_string()
                .contains("最新已明确失败")
        );
        for (mode, status) in [
            ("stop", "failed"),
            ("stop", "running"),
            ("status", "passed"),
            ("recover", "passed"),
        ] {
            let mut changed = original.clone();
            changed["attempts"]
                .as_array_mut()
                .unwrap()
                .push(json!({"number":4,"stage":"runtime-target","mode":mode,"status":status}));
            assert!(
                stages(&changed)
                    .unwrap_err()
                    .to_string()
                    .contains("最新精确stop")
            );
        }
        let mut no_failed_start = original.clone();
        no_failed_start["attempts"][1]["status"] = json!("passed");
        assert!(
            stages(&no_failed_start)
                .unwrap_err()
                .to_string()
                .contains("最新已明确失败")
        );
    }
}
