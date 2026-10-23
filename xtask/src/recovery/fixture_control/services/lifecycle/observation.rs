//! 服务状态由 Rust 结合精确身份、端口与完整成员证据判定。
use super::*;
use crate::recovery::clone::native::resources::common;

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
    let cache = runtime.call(
        "cache-status",
        json!({"request":services["requests"]["redis"],
        "runtime":services["runtime"],"effective":services["cache_effective"]}),
    )?;
    check(
        ["running", "stopped"]
            .iter()
            .any(|state| cache["state"] == *state),
        "Redis 状态观察未返回明确结果",
    )?;
    let tree = &services["tree"];
    let request = &services["requests"]["rustfs"];
    let identities = identities(runtime, tree)?;
    let all = |expected: &str| {
        ["supervisor", "monitor", "process"]
            .iter()
            .all(|role| identities[*role] == expected)
    };
    let (rustfs, termination) = if all("running") {
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
        if stopped.exists() {
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
    check(
        termination.is_null() || cache["state"] == "stopped",
        "RustFS 外部终止时 Redis 仍存活或状态未知",
    )?;
    Ok(json!({"redis":cache["state"],"rustfs":rustfs,"termination":termination}))
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

fn closed(request: &Value) -> Result<()> {
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
    check(
        read(&ready)? == expected,
        "RustFS 成员监督器启动归属证据缺失或变化",
    )?;
    let directory = Path::new(text(tree, "runtime_directory")?);
    for suffix in ["control", "result"] {
        check(
            !directory
                .join(format!(
                    "rustfs-tree-{}-{suffix}.json",
                    text(tree, "operation_id")?
                ))
                .exists(),
            "RustFS 外部终止现场包含未完成的控制或关闭证据",
        )?;
    }
    closed(&services["requests"]["rustfs"])?;
    check(
        identities(runtime, tree)? == *observations,
        "RustFS 外部终止核对期间进程身份变化",
    )?;
    let mut evidence = services["evidence"]
        .as_object()
        .ok_or("服务观察缺少证据对象")?
        .clone();
    evidence.insert("rustfs_monitor_ready".into(), binding(&ready)?);
    Ok(
        json!({"status":"external-termination-unreconciled","identities":observations,
        "normal_shutdown_proof":null,"evidence":evidence}),
    )
}
