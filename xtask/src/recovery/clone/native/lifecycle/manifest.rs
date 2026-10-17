use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{Map, Value, json};

use super::super::{
    digest::{self, FileDigest},
    json::{self as json_io, exact, text},
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

const BASE_FIELDS: &[&str] = &[
    "format_version",
    "kind",
    "id",
    "source_request",
    "source_export",
    "initialized",
    "source_environment",
    "target_environment",
    "copy_directory",
    "copy_stage",
    "build_bridges",
];

pub(crate) struct Manifest {
    pub(crate) value: Value,
    pub(crate) digest: FileDigest,
}

pub(crate) fn load(backend: &Path, path: &Path) -> Result<Manifest> {
    validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("复制运行清单无效：{error}"))?;
    let before = digest::file(path)?;
    let value = json_io::read(path)?;
    validate(&value, backend)?;
    if digest::file(path)? != before {
        return Err("复制运行清单读取期间发生变化".into());
    }
    Ok(Manifest {
        value,
        digest: before,
    })
}

pub(crate) fn validate(value: &Value, backend: &Path) -> Result<()> {
    let copy_stage = value
        .get("copy_stage")
        .and_then(Value::as_str)
        .ok_or("复制运行清单缺少 copy_stage")?;
    if !matches!(copy_stage, "source_to_seed" | "seed_to_arm") {
        return Err("复制阶段必须明确为来源到 seed 或 seed 到测量侧".into());
    }
    if copy_stage == "seed_to_arm" {
        return Err("seed_to_arm 生命周期必须整体使用 Python 状态机".into());
    }
    let root = exact(value, BASE_FIELDS, "复制运行清单")?;
    if root.get("format_version").and_then(Value::as_u64) != Some(1)
        || root.get("kind").and_then(Value::as_str) != Some("devex-clone-run")
    {
        return Err("统一验收清单类型无效".into());
    }
    super::super::input::name(text(root.get("id").unwrap(), "复制运行 ID")?)?;
    let _ = bound_file(backend, root.get("source_request").unwrap(), "源请求", true)?;
    for name in ["initialized", "source_environment", "target_environment"] {
        bound_file(backend, root.get(name).unwrap(), name, true)?;
    }
    if !root.get("source_export").unwrap().is_null() {
        let source_export =
            bound_file(backend, root.get("source_export").unwrap(), "源导出", true)?;
        let exported = json_io::read(&source_export)?;
        if exported.get("request") != root.get("source_request") {
            return Err("导出证据不属于固定源请求".into());
        }
    }
    let copy_directory = PathBuf::from(text(root.get("copy_directory").unwrap(), "复制目录")?);
    validate_local_test_path(&copy_directory, backend, LocalTestPathKind::StateDirectory)
        .map_err(|error| format!("复制目录无效：{error}"))?;
    validate_bridges(backend, root.get("build_bridges").unwrap())?;
    Ok(())
}

fn validate_bridges(backend: &Path, value: &Value) -> Result<()> {
    let bridges = value.as_array().ok_or("构建审计证据必须是明确列表")?;
    let sha256 = Regex::new(r"^[a-f0-9]{64}$")?;
    for bridge in bridges {
        let bridge = exact(bridge, &["path", "sha256"], "构建审计证据")?;
        let path = Path::new(text(bridge.get("path").unwrap(), "构建审计路径")?);
        validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)
            .map_err(|error| format!("构建审计证据路径无效：{error}"))?;
        let sha = text(bridge.get("sha256").unwrap(), "构建审计摘要")?;
        if !sha256.is_match(sha) || digest::file(path)?.sha256 != sha {
            return Err("构建审计证据发生变化".into());
        }
    }
    Ok(())
}

pub(crate) fn binding(path: &Path, digest: &FileDigest) -> Result<Value> {
    Ok(json!({
        "path": path_text(path, "证据路径")?,
        "bytes": digest.bytes,
        "sha256": digest.sha256,
    }))
}

pub(crate) fn path_text(path: &Path, label: &str) -> Result<String> {
    let value = digest::path_text(path, label)?;
    #[cfg(windows)]
    let value = value.replace('/', "\\");
    Ok(value)
}

pub(crate) fn bound_file(
    backend: &Path,
    value: &Value,
    label: &str,
    existing: bool,
) -> Result<PathBuf> {
    let binding = validate_binding_shape(value, label)?;
    let path = PathBuf::from(text(binding.get("path").unwrap(), label)?);
    let kind = if existing {
        LocalTestPathKind::ExistingFile
    } else {
        LocalTestPathKind::OutputFile
    };
    validate_local_test_path(&path, backend, kind)
        .map_err(|error| format!("{label}无效：{error}"))?;
    if existing {
        let digest = digest::file(&path)?;
        if Some(digest.bytes) != binding.get("bytes").and_then(Value::as_u64)
            || Some(digest.sha256.as_str()) != binding.get("sha256").and_then(Value::as_str)
        {
            return Err(format!("{label}与登记摘要不一致").into());
        }
    }
    Ok(path)
}

pub(crate) fn validate_binding_shape<'a>(
    value: &'a Value,
    label: &str,
) -> Result<&'a Map<String, Value>> {
    let binding = exact(value, &["path", "bytes", "sha256"], label)?;
    if binding
        .get("bytes")
        .and_then(Value::as_u64)
        .is_none_or(|bytes| bytes == 0)
        || binding
            .get("sha256")
            .and_then(Value::as_str)
            .is_none_or(|sha| !Regex::new(r"^[a-f0-9]{64}$").is_ok_and(|re| re.is_match(sha)))
    {
        return Err(format!("{label}大小或摘要无效").into());
    }
    Ok(binding)
}

pub(crate) fn overlaps(first: &Path, second: &Path) -> bool {
    first == second || first.starts_with(second) || second.starts_with(first)
}

pub(crate) fn target_storage_path(backend: &Path, value: &Value) -> Result<Option<PathBuf>> {
    if value.get("copy_stage").and_then(Value::as_str) != Some("seed_to_arm") {
        return Ok(None);
    }
    let descriptor = value
        .get("target_storage_run")
        .ok_or("seed_to_arm 清单缺少 target_storage_run")?;
    let descriptor = exact(
        descriptor,
        &["path", "manifest", "storage", "cache"],
        "target_storage_run",
    )?;
    let path = PathBuf::from(text(
        descriptor.get("path").unwrap(),
        "target_storage_run 路径",
    )?);
    validate_local_test_path(&path, backend, LocalTestPathKind::ExistingDirectory)
        .map_err(|error| format!("target_storage_run 路径无效：{error}"))?;
    Ok(Some(path))
}
