//! 已知停机与启动失败必须有完整控制器、调用位置和精确回收证据。
use super::super::{lineage::failure::inputs, stops};
use crate::{
    Result,
    cli::CloneSide,
    recovery::clone::native::{
        json as document,
        resources::{
            common,
            storage::{model, process, registered},
        },
    },
};
use serde_json::{Value, json};
use std::path::Path;

#[path = "restart_contract.rs"]
mod restart_contract;

pub(super) fn stop(
    backend: &Path,
    run: &Path,
    row: &Value,
    previous: &Value,
    first: u64,
) -> Result<Vec<Value>> {
    if row["status"] != "failed"
        || !row["result"].is_null()
        || row["error_type"] != "ValueError"
        || row["sources"] != previous["sources"]
    {
        return Err("分段续作只接受同一来源的固定存储停止失败".into());
    }
    let (failure, controller) = incident(
        backend,
        run,
        row,
        "stop",
        "clone.storage-target.stop",
        &[
            ("devex_clone_run", "execute"),
            ("devex_clone_storage", "execute_storage"),
            ("devex_clone_storage", "observe_or_stop"),
            ("devex_clone_storage_process", "stop_record"),
            ("devex_clone_source_proof", "require_closed_port"),
            ("process_sockets", "verify_windows_port_idle"),
        ],
    )?;
    let owner = common::read_bound(backend, &controller, "失败存储停止控制器")?["owner"].clone();
    common::require_recorded_stopped(&owner["identity"])?;
    super::product(&row["sources"], row)?;
    let output = run.join(format!("storage-target/a{:04}", super::number(row)?));
    common::local_directory(backend, &output, "失败存储停止输出")?;
    let entries = crate::recovery::runtime::dataset_authority::reads::entries(&output)?
        .collect::<std::io::Result<Vec<_>>>()?;
    let expected = output.join(format!("p{first:04}"));
    if entries.len() != 1 || entries[0].path() != expected {
        return Err("失败存储停止留下未知输出或触碰其他代次".into());
    }
    common::local_directory(backend, &expected, "失败存储停止首代观察")?;
    if crate::recovery::runtime::dataset_authority::reads::entries(expected)?
        .next()
        .is_some()
    {
        return Err("失败存储停止留下未知输出或触碰其他代次".into());
    }
    Ok(vec![failure, controller])
}

pub(super) fn restart(
    backend: &Path,
    run: &Path,
    row: &Value,
    expected: &Value,
    idle: bool,
) -> Result<Vec<Value>> {
    let (failure, controller) = restart_incident(backend, run, row)?;
    let owner = common::read_bound(backend, &controller, "失败存储重启控制器")?["owner"].clone();
    let (request, descriptor) = registered::registration(backend, run, CloneSide::Target)?;
    if descriptor != *expected {
        return Err("失败存储重启没有消费相同固定请求".into());
    }
    let manifest = document::read(&run.join("manifest.json"))?;
    let private = model::validate(backend, run, &manifest, CloneSide::Target, &request)?.private;
    stops::directory_identity(backend, &request["data_directory"])?;
    let number = super::number(row)?;
    let output = run.join(format!("storage-target/a{number:04}"));
    let frozen = files(backend, &output)?;
    let observed = process::inspect_attempt(backend, &output, &descriptor, &controller, number)?
        .ok_or("失败存储重启缺少创建证据")?;
    if observed["state"] != "recorded"
        || observed["process_receipt"] != common::binding(&output.join("process.json"))?
        || observed["launch_receipt"] != common::binding(&output.join("launch.json"))?
    {
        return Err("失败存储重启缺少完整创建身份和启动参数".into());
    }
    let identity = exact_cleanup(&output, &observed)?;
    common::require_recorded_stopped(&owner["identity"])?;
    common::require_recorded_stopped(&identity)?;
    if idle {
        for key in ["api_url", "console_url"] {
            common::require_closed_port(document::text(&request[key], "失败重启端口")?)?;
        }
    }
    if files(backend, &output)? != frozen
        || registered::registration(backend, run, CloneSide::Target)?
            != (request.clone(), descriptor)
        || model::validate(
            backend,
            run,
            &document::read(&run.join("manifest.json"))?,
            CloneSide::Target,
            &request,
        )?
        .private
            != private
    {
        return Err("失败存储重启证据、请求或数据目录在核验期间变化".into());
    }
    stops::directory_identity(backend, &request["data_directory"])?;
    for binding in [&failure, &controller] {
        common::bound_file(backend, binding, "失败存储重启原证据")?;
    }
    Ok(vec![failure, controller])
}

pub(in crate::recovery::source::evidence) fn restart_request(args: &Value) -> Result<Value> {
    document::exact(args, &["directory", "attempt"], "历史存储重启失败核验参数")?;
    let backend = crate::workspace::root_dir();
    let run = Path::new(document::text(&args["directory"], "历史运行目录")?);
    common::local_directory(&backend, run, "历史存储重启运行")?;
    let state_path = run.join("state.json");
    let state_binding = common::binding(&state_path)?;
    let manifest = common::binding(&run.join("manifest.json"))?;
    let state = crate::recovery::clone::native::lifecycle::state::load(run, true)?;
    let number = super::number(&args["attempt"])?;
    let row = document::array(&state["attempts"], "历史阶段")?
        .iter()
        .find(|row| row["number"].as_u64() == Some(number))
        .ok_or("历史存储重启阶段不存在；核对原始运行和阶段编号")?;
    if *row != args["attempt"] {
        return Err("历史存储重启参数与实际完整阶段不同；重新读取原始 state 后核验".into());
    }
    let (failure, controller) = restart_incident(&backend, run, row)?;
    for binding in [&state_binding, &manifest, &failure, &controller] {
        common::bound_file(&backend, binding, "历史存储失败核验前后绑定")?;
    }
    Ok(json!({"failure":failure,"controller":controller}))
}

fn restart_incident(backend: &Path, run: &Path, row: &Value) -> Result<(Value, Value)> {
    if row["stage"] != "storage-target"
        || row["mode"] != "restart"
        || row["status"] != "failed"
        || !row["result"].is_null()
    {
        return Err("历史记录不是已失败且未发布结果的存储重启；核对完整原始阶段".into());
    }
    let initial = super::super::lineage::failure::controller(run, row)?;
    let contract = restart_contract::restart_contract(backend, row)?;
    if row["error_type"] != contract.error_type {
        return Err("历史存储失败类型与已绑定 producer 合同不同；禁止混用新旧证据".into());
    }
    let observed = incident(
        backend,
        run,
        row,
        "restart",
        "clone.storage-target.restart",
        &contract.calls,
    )?;
    if observed.1 != initial.0 || super::super::lineage::failure::controller(run, row)? != initial {
        return Err("历史存储控制器在合同核验期间变化；保留现场重新核对".into());
    }
    for binding in [&observed.0, &observed.1] {
        common::bound_file(backend, binding, "历史存储失败原始绑定")?;
    }
    Ok(observed)
}

fn files(backend: &Path, output: &Path) -> Result<Value> {
    let names = [
        "intent.json",
        "spawned.json",
        "process.json",
        "launch.json",
        "failure-cleanup.json",
        "stdout.log",
        "stderr.log",
    ];
    inputs::exact_directory(backend, output, &names, &[])?;
    let mut result = serde_json::Map::new();
    for name in names {
        result.insert(name.into(), common::binding(&output.join(name))?);
    }
    Ok(Value::Object(result))
}

fn incident(
    backend: &Path,
    run: &Path,
    row: &Value,
    mode: &str,
    operation: &str,
    calls: &[(&str, &str)],
) -> Result<(Value, Value)> {
    let number = super::number(row)?;
    let (controller, _) = super::super::lineage::failure::controller(run, row)?;
    let binding = common::binding(&run.join(format!("failure-{number:04}.json")))?;
    let value = common::read_bound(backend, &binding, "分段原失败")?;
    let fields = [
        "kind",
        "attempt",
        "stage",
        "mode",
        "error_type",
        "controller",
    ];
    let mut expected = fields.to_vec();
    expected.push("format_version");
    match value["format_version"].as_u64() {
        Some(1) => {
            expected.push("frames");
            document::exact(&value, &expected, "历史 Python 分段失败")?;
            let frames = document::array(&value["frames"], "历史失败调用位置")?;
            if frames.len() != calls.len() {
                return Err("历史分段失败调用链不完整".into());
            }
            for (frame, (file, function)) in frames.iter().zip(calls) {
                document::exact(frame, &["file", "function", "line"], "历史失败位置")?;
                if frame["file"] != format!("tools/python/{file}.py")
                    || frame["function"] != *function
                    || frame["line"].as_u64().is_none_or(|n| n == 0)
                {
                    return Err("历史分段失败调用链不匹配".into());
                }
            }
        }
        Some(2) => {
            expected.push("operation_id");
            document::exact(&value, &expected, "原生分段失败")?;
            if value["operation_id"] != operation {
                return Err("原生分段失败操作不匹配".into());
            }
        }
        _ => return Err("分段失败格式无效".into()),
    }
    if value["kind"] != "devex-stage-failure"
        || value["attempt"] != row["number"]
        || value["stage"] != "storage-target"
        || value["mode"] != mode
        || value["error_type"] != row["error_type"]
        || value["controller"] != controller
    {
        return Err("存储失败不是冻结的控制器和操作调用链".into());
    }
    let owner = common::read_bound(backend, &controller, "分段原控制器")?["owner"].clone();
    crate::recovery::clone::native::lifecycle::identity::validate(&owner, run)?;
    if owner["manifest_sha256"] != common::binding(&run.join("manifest.json"))?["sha256"] {
        return Err("失败存储控制器不属于固定运行".into());
    }
    Ok((binding, controller))
}

fn exact_cleanup(output: &Path, observed: &Value) -> Result<Value> {
    let spawned = document::read(&output.join("spawned.json"))?;
    let cleanup = document::read(&output.join("failure-cleanup.json"))?;
    document::exact(&spawned, &["pid", "identity_pending"], "失败启动 PID")?;
    document::exact(
        &cleanup,
        &["intent", "identity", "pid", "returncode"],
        "失败启动回收",
    )?;
    let identity = common::validate_process_identity(&observed["identity"])?;
    if spawned != json!({"pid":identity["pid"],"identity_pending":true})
        || cleanup
            != json!({"intent":observed["intent"],"identity":identity,"pid":identity["pid"],"returncode":0})
    {
        return Err("失败存储重启的创建、回收与退出身份不一致".into());
    }
    Ok(identity)
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/recovery/source/runtime/history/segment_failures.rs"]
mod tests;
