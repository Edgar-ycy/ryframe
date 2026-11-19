//! 服务状态由 Rust 结合精确身份、端口与完整成员证据判定。
use super::*;
use crate::recovery::clone::native::resources::common;

#[path = "observation/incident.rs"]
mod incident;
pub(super) use incident::{rustfs_reboot_evidence, rustfs_reboot_incident};

pub(super) fn observe(
    runtime: &mut dyn Runtime,
    backend: &Path,
    services: &Value,
) -> Result<Value> {
    common::local_directory(
        backend,
        Path::new(text(&services["tree"], "runtime_directory")?),
        "服务观察目录",
    )?;
    let tree = &services["tree"];
    let request = &services["requests"]["rustfs"];
    let identities = identities(runtime, tree)?;
    let all = |expected: &str| {
        ["supervisor", "monitor", "process"]
            .iter()
            .all(|role| identities[*role] == expected)
    };
    let (rustfs, mut termination) = if all("running") {
        check(
            runtime.call(
                "storage-running",
                json!({"identity":tree["process"],"request":request}),
            )? == true,
            "RustFS 完整进程树观察期间产品进程退出",
        )?;
        ("running", Value::Null)
    } else if all("missing") {
        let stopped = receipt(tree, "stopped")?;
        if entry_exists(&stopped)? {
            runtime.call("members-wait", json!({"tree":tree}))?;
            closed(request)?;
            ("stopped", Value::Null)
        } else {
            (
                "external-termination-unreconciled",
                external(runtime, services, &identities)?,
            )
        }
    } else {
        return Err(model::error("RustFS 进程树仅部分存活，不能判定服务状态"));
    };
    let cache = cache_state(runtime, services, &identities, &mut termination)?;
    Ok(json!({"redis":cache,"rustfs":rustfs,"termination":termination}))
}

fn cache_state(
    runtime: &mut dyn Runtime,
    services: &Value,
    identities: &Value,
    termination: &mut Value,
) -> Result<Value> {
    let args = json!({"request":services["requests"]["redis"],
        "runtime":services["runtime"],"effective":services["cache_effective"]});
    if !termination.is_null() && args["request"]["format_version"] == 3 {
        let mut redis = runtime.call("cache-external-termination", args.clone())?;
        if !redis.is_null() {
            let recorded = &termination["evidence"]["redis_runtime"];
            check(
                redis["kind"] == "windows-redis-external-termination"
                    && redis
                        .get("normal_job_exit_proof")
                        .is_some_and(Value::is_null)
                    && Path::new(text(&redis["runtime"], "path")?)
                        == Path::new(text(recorded, "path")?)
                    && redis["runtime"]["sha256"] == recorded["sha256"]
                    && redis["runtime"]["bytes"] == recorded["bytes"]
                    && external(runtime, services, identities)? == *termination,
                "Redis 外部终止证据或原 RustFS 终止状态变化",
            )?;
            redis["runtime"] = recorded.clone();
            termination["evidence"]["redis_external"] = redis;
            return Ok(json!("stopped"));
        }
    }
    let cache = runtime.call("cache-status", args)?;
    check(
        ["running", "stopped"]
            .iter()
            .any(|state| cache["state"] == *state),
        "Redis 状态观察未返回明确结果",
    )?;
    check(
        termination.is_null() || cache["state"] == "stopped",
        "RustFS 外部终止时 Redis 仍存活或状态未知",
    )?;
    Ok(cache["state"].clone())
}

pub(super) fn identities(runtime: &mut dyn Runtime, tree: &Value) -> Result<Value> {
    let deadline = runtime.now() + Duration::from_secs(5);
    let mut observations = json!({});
    for role in ["supervisor", "monitor", "process"] {
        let expected = &tree[role];
        common::validate_process_identity(expected)?;
        let actual = loop {
            match runtime.call("identity", json!({"pid":expected["pid"]})) {
                Err(error)
                    if model::category(error.as_ref()) == "PermissionError"
                        && runtime.now() < deadline =>
                {
                    runtime.pause(Duration::from_millis(50));
                }
                result => break result?,
            }
        };
        if !actual.is_null() {
            common::validate_process_identity(&actual)?;
        }
        // PID 被复用时，被记录身份已消失；任何终止仍须核对原始完整身份。
        observations[role] = json!(if actual == *expected {
            "running"
        } else {
            "missing"
        });
    }
    Ok(observations)
}

pub(super) fn receipt(tree: &Value, suffix: &str) -> Result<PathBuf> {
    let operation = text(tree, "operation_id")?;
    check(
        operation.len() == 32
            && operation
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "进程树操作 ID 无效",
    )?;
    check(tree["role"] == "rustfs", "服务观察只接受 RustFS 进程树")?;
    Ok(Path::new(text(tree, "runtime_directory")?)
        .join(format!("rustfs-members-{operation}-{suffix}.json")))
}

pub(super) fn closed(request: &Value) -> Result<()> {
    for key in ["api_url", "console_url"] {
        common::require_closed_port(text(request, key)?)?;
    }
    Ok(())
}

fn external(runtime: &mut dyn Runtime, services: &Value, observations: &Value) -> Result<Value> {
    let tree = &services["tree"];
    let ready = receipt(tree, "ready")?;
    let expected = json!({"format_version":1,"operation_id":tree["operation_id"],
        "directory":tree["runtime_directory"],"role":tree["role"],"scope_id":tree["scope_id"],
        "supervisor":tree["supervisor"],"monitor":tree["monitor"]});
    let ready_binding = binding(&ready)?;
    check(
        read(&ready)? == expected,
        "RustFS 成员监督器启动归属证据缺失或变化",
    )?;
    no_close_receipts(tree)?;
    closed(&services["requests"]["rustfs"])?;
    check(
        identities(runtime, tree)? == *observations && binding(&ready)? == ready_binding,
        "RustFS 外部终止核对期间进程身份变化或启动归属证据变化",
    )?;
    no_close_receipts(tree)?;
    let mut evidence = services["evidence"]
        .as_object()
        .ok_or("服务观察缺少证据对象")?
        .clone();
    evidence.insert("rustfs_monitor_ready".into(), ready_binding);
    Ok(
        json!({"status":"external-termination-unreconciled","identities":observations,
        "normal_shutdown_proof":null,"evidence":evidence}),
    )
}

fn no_close_receipts(tree: &Value) -> Result<()> {
    let directory = Path::new(text(tree, "runtime_directory")?);
    for entry in std::fs::read_dir(directory)? {
        let name = entry?.file_name();
        let name = name.to_str().ok_or("RustFS 控制目录文件名不是 UTF-8")?;
        let receipt = name.trim_start_matches('.').to_ascii_lowercase();
        let close = (receipt.starts_with("rustfs-tree-")
            && ["-control.json", "-result.json"]
                .iter()
                .any(|suffix| receipt.contains(suffix)))
            || receipt.starts_with("rustfs-members-") && receipt.contains("-stopped.json")
            || ["actor-cancel.json", "actor-failed.json"]
                .iter()
                .any(|prefix| receipt.starts_with(prefix));
        check(!close, "RustFS 外部终止现场包含未完成的控制或关闭证据")?;
    }
    Ok(())
}

fn entry_exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}
