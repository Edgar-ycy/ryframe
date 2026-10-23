//! 复用正式构建验证，再比较允许归一化后的完整收据。
use super::{Path, Result, Value, bridge, common, document, maintenance};

pub(super) fn verify(backend: &Path, previous: &Value, request: &Value) -> Result<()> {
    let old_runtime_path =
        common::bound_file(backend, &previous["backend_build"], "原 API/Worker 构建")?;
    let new_runtime_path =
        common::bound_file(backend, &request["backend_build"], "当前 API/Worker 构建")?;
    let old_runtime = document::read(&old_runtime_path)?;
    let new_runtime = document::read(&new_runtime_path)?;
    for (path, value) in [
        (&old_runtime_path, &old_runtime),
        (&new_runtime_path, &new_runtime),
    ] {
        if value["kind"] != "restore-backend-build" {
            return Err("重新登记的 API/Worker 构建类型无效".into());
        }
        bridge::verify_build(backend, path, value)?;
    }
    equivalent_runtime(&old_runtime, &new_runtime, request)?;
    let execution = Path::new(document::text(
        &request["execution_backend"],
        "维护执行根目录",
    )?);
    crate::local_test_path::validate_local_test_path(
        execution,
        backend,
        crate::local_test_path::LocalTestPathKind::ExistingDirectory,
    )?;
    let old_path = common::bound_file(execution, &previous["maintenance_build"], "原维护构建")?;
    let new_path = common::bound_file(execution, &request["maintenance_build"], "当前维护构建")?;
    let old = maintenance::verify_evidence(execution, &old_path, None)?;
    let new = maintenance::verify_evidence(execution, &new_path, None)?;
    equivalent_maintenance(&old_runtime, &old, &old_path, &new, &new_path, request)?;
    for (root, descriptor) in [
        (backend, &previous["backend_build"]),
        (backend, &request["backend_build"]),
        (execution, &previous["maintenance_build"]),
        (execution, &request["maintenance_build"]),
    ] {
        common::bound_file(root, descriptor, "生成重新登记构建")?;
    }
    Ok(())
}

pub(super) fn equivalent_runtime(old: &Value, new: &Value, request: &Value) -> Result<()> {
    let mut stable = [old.clone(), new.clone()];
    for value in &mut stable {
        clean_source(&value["sources"]["full"]["source"], request)?;
        document::exact(
            &value["build"]["environment"],
            &["variables", "sha256"],
            "构建环境",
        )?;
        value["build"]["environment"]["sha256"] = "<current-environment>".into();
    }
    if stable[0] != stable[1] {
        return Err("重新登记的 API/Worker 构建改变了源码、命令、工具链或产物".into());
    }
    Ok(())
}

fn clean_source(source: &Value, request: &Value) -> Result<()> {
    let snapshot = &source["snapshot"];
    if request["expected_backend_sha"].as_str().is_none()
        || snapshot["head"] != request["expected_backend_sha"]
        || snapshot["clean"].as_bool() != Some(true)
        || snapshot["files"]
            .as_array()
            .is_none_or(|files| !files.is_empty())
    {
        return Err("重新登记的构建不是同一干净后端来源".into());
    }
    Ok(())
}

pub(super) fn equivalent_maintenance(
    runtime: &Value,
    old: &Value,
    old_path: &Path,
    new: &Value,
    new_path: &Path,
    request: &Value,
) -> Result<()> {
    for value in [old, new] {
        clean_source(&value["source"], request)?;
        if value["backend_root"] != request["execution_backend"]
            || value["source_inventory"] != runtime["sources"]["full"]
            || value["source"] != runtime["sources"]["full"]["source"]
        {
            return Err("重新登记的维护构建不是同一干净后端来源".into());
        }
    }
    if stable_maintenance(old, old_path)? != stable_maintenance(new, new_path)? {
        return Err("重新登记的维护构建改变了源码、命令、工具链或产物".into());
    }
    Ok(())
}

fn stable_maintenance(value: &Value, receipt: &Path) -> Result<Value> {
    let mut stable = value.clone();
    document::exact(
        &stable["artifacts"],
        &["reset", "migrate", "tenant-data"],
        "维护产物",
    )?;
    for artifact in stable["artifacts"]
        .as_object_mut()
        .ok_or("维护产物无效")?
        .values_mut()
    {
        let executable = Path::new(document::text(&artifact["executable"], "维护产物路径")?);
        if executable.parent() != receipt.parent() {
            return Err("维护构建产物不属于自身收据目录".into());
        }
        artifact["executable"] = executable
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("维护产物名无效")?
            .into();
        for field in ["cargo_output", "cargo_log"] {
            document::exact(
                &artifact[field],
                &["file", "bytes", "sha256"],
                "维护构建日志",
            )?;
            artifact[field] = serde_json::json!({"file":artifact[field]["file"]});
        }
    }
    Ok(stable)
}
