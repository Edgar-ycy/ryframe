//! 单个 CycloneDX 报告的只读校验，不加载源码策略或执行外部工具。

use std::{fs, path::Path};

use serde_json::Value;

use crate::{Result, process::PreservedFailure};

pub(crate) fn validate(source: &str, require_reproducible: bool) -> Vec<String> {
    let document = match serde_json::from_str::<Value>(source) {
        Ok(document) if document.is_object() => document,
        _ => return vec!["无法读取 CycloneDX 清单：必须是 JSON 对象".to_owned()],
    };
    let mut errors = Vec::new();
    if document.get("bomFormat").and_then(Value::as_str) != Some("CycloneDX") {
        errors.push("SBOM 的 bomFormat 必须是 CycloneDX".to_owned());
    }
    if !matches!(
        document.get("specVersion").and_then(Value::as_str),
        Some("1.5" | "1.6" | "1.7")
    ) {
        errors.push("SBOM 规范版本不得低于 1.5".to_owned());
    }
    if !document
        .get("components")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty())
    {
        errors.push("SBOM 必须包含非空 components".to_owned());
    }
    if !document
        .get("metadata")
        .and_then(|metadata| metadata.get("component"))
        .is_some_and(Value::is_object)
    {
        errors.push("SBOM 必须声明顶层 metadata.component".to_owned());
    }
    if require_reproducible
        && document
            .get("serialNumber")
            .is_some_and(|serial| !serial.is_null())
    {
        errors.push("可复现 SBOM 不得包含随机 serialNumber".to_owned());
    }
    errors
}

pub(crate) fn check(path: &Path, require_reproducible: bool) -> Result<Vec<String>> {
    let source = fs::read_to_string(path).map_err(|_| {
        // 文件路径、JSON 片段和底层异常均可能携带凭据，诊断只报告固定原因。
        PreservedFailure::new(
            "无法读取 CycloneDX 清单：文件不可读取或 UTF-8 无效".to_owned(),
            Some(1),
        )
    })?;
    Ok(validate(&source, require_reproducible))
}

pub(crate) fn run(path: &Path, require_reproducible: bool) -> Result<()> {
    let errors = check(path, require_reproducible)?;
    if !errors.is_empty() {
        return Err(PreservedFailure::new(errors.join("\n"), Some(1)).into());
    }
    println!("CycloneDX 报告校验通过");
    Ok(())
}
