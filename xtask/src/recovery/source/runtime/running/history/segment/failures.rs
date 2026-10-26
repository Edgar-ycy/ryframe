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
    let entries = std::fs::read_dir(&output)?.collect::<std::io::Result<Vec<_>>>()?;
    let expected = output.join(format!("p{first:04}"));
    if entries.len() != 1 || entries[0].path() != expected {
        return Err("失败存储停止留下未知输出或触碰其他代次".into());
    }
    common::local_directory(backend, &expected, "失败存储停止首代观察")?;
    if std::fs::read_dir(expected)?.next().is_some() {
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
    if row["status"] != "failed"
        || !row["result"].is_null()
        || row["error_type"] != "CalledProcessError"
    {
        return Err("分段续作只接受已精确回收的存储身份查询失败".into());
    }
    let (failure, controller) = incident(
        backend,
        run,
        row,
        "restart",
        "clone.storage-target.restart",
        &[
            ("devex_clone_run", "execute"),
            ("devex_clone_storage", "execute_storage"),
            ("devex_clone_storage_process", "start"),
            ("devex_clone_storage_process", "actual_arguments"),
            ("devex_clone_target_storage", "actual_windows_argv"),
        ],
    )?;
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
    let controller = registered::controller_binding(run, row)?;
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
