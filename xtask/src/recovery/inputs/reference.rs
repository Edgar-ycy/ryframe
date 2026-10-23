use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value, json};

use super::{
    arm,
    document::object,
    domain::{identifier, validate_plan},
};
use crate::{Result, cli::ReferenceInputOptions};

pub(crate) fn build(backend: &Path, options: &ReferenceInputOptions) -> Result<Value> {
    identifier(&options.id)?;
    let (arm, _fresh) = arm::load(
        backend,
        &options.arm_input,
        &options.fresh_target_verify,
        options.side.as_str(),
    )?;
    let source_request = object(&arm.source_request.value, "共享源导出请求")?;
    let request = object(&arm.target.document.value, "新目标请求")?;
    let request_target = object(
        request.get("target").ok_or("新目标请求缺少 target")?,
        "新目标",
    )?;
    let selected = object(&arm.target.selected, "审阅单侧资源")?;
    let mut target = request_target.clone();
    for field in ["runtime_dir", "api_url", "worker_ready_url", "frontend_url"] {
        target.insert(
            field.to_owned(),
            selected
                .get(field)
                .cloned()
                .ok_or_else(|| format!("审阅单侧资源缺少 {field}"))?,
        );
    }
    let configuration = super::reference_configuration::Projection::load(backend, &arm)?;
    let source = configuration.source.source.clone();
    target.insert("s3".into(), configuration.target.target["s3"].clone());
    let directory = new_path(backend, &options.work_dir, "参考计划 work_dir")?;
    independent_work_dir(
        &directory,
        source_request,
        selected,
        &arm,
        &options.fresh_target_verify,
    )?;
    let plan = json!({
        "format_version": 1,
        "id": options.id,
        "target_side": options.side.as_str(),
        "work_dir": directory.to_string_lossy(),
        "tools": configuration.source.tools,
        "source": source,
        "target": Value::Object(target),
    });
    validate_plan(&plan, backend)?;
    if request_target.get("scope_id") != plan["target"].get("scope_id")
        || request_target.get("databases") != plan["target"].get("databases")
    {
        return Err("参考计划没有绑定当前 source-generation 或 fresh 物理目标".into());
    }
    configuration.assert_unchanged(backend, &arm)?;
    arm.target.document.assert_unchanged()?;
    arm.result.assert_unchanged()?;
    arm.request.assert_unchanged()?;
    arm.manifest.assert_unchanged()?;
    arm.source_request.assert_unchanged()?;
    Ok(plan)
}

fn new_path(backend: &Path, value: &Path, label: &str) -> Result<PathBuf> {
    if value.exists() {
        return Err(format!("{label}已存在，拒绝覆盖").into());
    }
    let parent = value.parent().ok_or_else(|| format!("{label}缺少父目录"))?;
    if !parent.is_dir() {
        return Err(format!("{label}的父目录必须已经存在").into());
    }
    let parent = plain_path(fs::canonicalize(parent)?);
    let name = value
        .file_name()
        .ok_or_else(|| format!("{label}缺少文件名"))?;
    let result = parent.join(name);
    if !result.starts_with(plain_path(fs::canonicalize(backend.join(".local-tests"))?)) {
        return Err(format!("{label}必须位于当前后端 .local-tests").into());
    }
    Ok(result)
}

fn independent_work_dir(
    directory: &Path,
    source_request: &Map<String, Value>,
    selected: &Map<String, Value>,
    arm: &arm::Arm,
    fresh_path: &Path,
) -> Result<()> {
    let mut protected = Vec::new();
    if let Some(value) = source_request
        .get("source")
        .and_then(|value| value.get("runtime_dir"))
        .and_then(Value::as_str)
    {
        protected.push(PathBuf::from(value));
    }
    if let Some(value) = selected.get("runtime_dir").and_then(Value::as_str) {
        protected.push(PathBuf::from(value));
    }
    protected.push(
        fresh_path
            .parent()
            .ok_or("fresh target 核验缺少父目录")?
            .to_path_buf(),
    );
    if let Some(value) = arm
        .manifest
        .value
        .get("copy_directory")
        .and_then(Value::as_str)
    {
        protected.push(PathBuf::from(value));
    }
    let resolved = plain_path(fs::canonicalize(
        directory.parent().ok_or("工作目录缺少父目录")?,
    )?)
    .join(directory.file_name().ok_or("工作目录缺少名称")?);
    for path in protected {
        let path = plain_path(fs::canonicalize(&path).unwrap_or(path));
        if overlaps(&resolved, &path) {
            return Err("参考计划 work_dir 必须与来源、目标、复制和核验目录完全分离".into());
        }
    }
    Ok(())
}

fn overlaps(first: &Path, second: &Path) -> bool {
    first == second || first.starts_with(second) || second.starts_with(first)
}

fn plain_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    if let Some(value) = path.to_str().and_then(|value| {
        value
            .strip_prefix(r"\\?\UNC\")
            .map(|value| format!(r"\\{value}"))
            .or_else(|| value.strip_prefix(r"\\?\").map(ToOwned::to_owned))
    }) {
        return PathBuf::from(value);
    }
    path
}
