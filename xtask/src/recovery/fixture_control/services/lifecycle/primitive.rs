//! 进入服务原语前先由 Rust 限定操作、字段与本地路径。
use super::*;
use crate::local_test_path::{LocalTestPathKind, validate_local_test_path};

pub(super) fn validate(backend: &Path, operation: &str, args: &Value) -> Result<()> {
    let fields: &[&str] = match operation {
        "review-successor" => &["previous", "review"],
        "cache-status" | "cache-external-termination" => &["request", "runtime", "effective"],
        "cache-handoff-markers" => &["request", "runtime", "markers"],
        "storage-running" => &["identity", "request"],
        "storage-closed" => &["request"],
        "members-wait" => &["tree"],
        "controller" => &["run"],
        "identity" => &["pid"],
        "completion" => &["tree"],
        "tree-stop" => &["tree", "crash"],
        "tree-read" => &["output", "scope_id"],
        "recover-controller" => &["run", "owner"],
        "cache-stop" => &[
            "request", "private", "runtime", "output", "context", "running",
        ],
        "cache-start" => &["request", "private", "output", "context", "running"],
        "storage-start" => &[
            "request",
            "private",
            "output",
            "manifest",
            "controller",
            "number",
            "context",
            "running",
        ],
        _ => return Err(model::error("未知服务原语；不启动子进程")),
    };
    json_io::exact(args, fields, "服务原语参数")?;
    for key in ["run", "output"] {
        if let Some(value) = args.get(key) {
            validate_local_test_path(
                Path::new(value.as_str().ok_or("原语路径必须为字符串")?),
                backend,
                LocalTestPathKind::ExistingDirectory,
            )?;
        }
    }
    if let Some(context) = args.get("context") {
        check(
            Path::new(text(context, "backend")?) == backend,
            "服务上下文属于其他后端",
        )?;
        validate_local_test_path(
            Path::new(text(context, "run")?),
            backend,
            LocalTestPathKind::ExistingDirectory,
        )?;
    }
    if let Some(tree) = args.get("tree") {
        validate_local_test_path(
            Path::new(text(tree, "runtime_directory")?),
            backend,
            LocalTestPathKind::ExistingDirectory,
        )?;
    }
    if operation == "identity" {
        check(
            args["pid"]
                .as_u64()
                .is_some_and(|pid| pid > 1 && u32::try_from(pid).is_ok()),
            "进程 PID 必须为有效正整数",
        )?;
    }
    for key in ["settled", "crash"] {
        if let Some(value) = args.get(key) {
            check(value.is_boolean(), "服务原语标志必须为布尔值")?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn unknown_fields_escaped_paths_and_invalid_pid_fail_before_process_launch() {
    let backend = crate::workspace::root_dir();
    for (operation, args) in [
        ("context", json!({})),
        ("registered", json!({"context":{},"settled":true})),
        ("observe", json!({"services":{}})),
        ("identity", json!({"pid":true})),
        ("identity", json!({"pid":0})),
        ("identity", json!({"pid":12,"secret":"private-marker"})),
        ("storage-closed", json!({})),
        ("storage-closed", json!({"request":{},"unexpected":true})),
        ("controller", json!({"run":backend})),
    ] {
        assert!(validate(&backend, operation, &args).is_err());
    }
    assert!(validate(&backend, "identity", &json!({"pid":12})).is_ok());
}

#[cfg(test)]
#[test]
fn storage_start_requires_a_bound_local_output_and_context() {
    let backend = crate::workspace::root_dir();
    let local = backend.join(".local-tests");
    std::fs::create_dir_all(&local).unwrap();
    let run = tempfile::tempdir_in(local).unwrap();
    let output = run.path().join("storage");
    std::fs::create_dir(&output).unwrap();
    let args = json!({
        "request": {},
        "private": {},
        "output": output,
        "manifest": {},
        "controller": {},
        "number": 4,
        "context": {"backend": backend, "run": run.path()},
        "running": {}
    });
    assert!(validate(&backend, "storage-start", &args).is_ok());

    let mut escaped = args.clone();
    escaped["output"] = json!(backend);
    assert!(validate(&backend, "storage-start", &escaped).is_err());

    let mut foreign = args.clone();
    foreign["context"]["backend"] = json!(backend.parent().unwrap());
    assert!(validate(&backend, "storage-start", &foreign).is_err());

    let mut escaped_run = args;
    escaped_run["context"]["run"] = json!(backend);
    assert!(validate(&backend, "storage-start", &escaped_run).is_err());
}
