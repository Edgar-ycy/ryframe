use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::Result;

use super::super::{
    evidence::EMPTY_SHA256,
    support::{exact_object, sha256_shape},
};
use super::{
    ARCHIVE_RECEIPT_LIMITS, BUILD_EVIDENCE, BUSINESS_MODEL, FIXTURE_RECEIPT, FRONTEND_BUILD,
    FRONTEND_SOURCE, RUNTIME_EVIDENCE, RUNTIME_RECEIPT, SOURCE_PAIR_RECEIPT, clean_snapshot,
    file_name, generated_snapshot, join_posix, parent, validate_absolute_posix, validate_binding,
    validate_relative_member,
};

pub(super) fn archive_members(
    entries: &[(String, Vec<u8>)],
    fixture: &str,
) -> Result<BTreeMap<String, Vec<u8>>> {
    if !matches!(fixture, "core" | "business") {
        return Err("全栈归档 fixture 必须是 core 或 business".into());
    }
    let expected = if fixture == "business" {
        BTreeSet::from([
            SOURCE_PAIR_RECEIPT,
            BUILD_EVIDENCE,
            RUNTIME_RECEIPT,
            RUNTIME_EVIDENCE,
            FIXTURE_RECEIPT,
            FRONTEND_SOURCE,
            FRONTEND_BUILD,
        ])
    } else {
        BTreeSet::from([
            SOURCE_PAIR_RECEIPT,
            BUILD_EVIDENCE,
            RUNTIME_RECEIPT,
            RUNTIME_EVIDENCE,
        ])
    };
    let mut selected = BTreeMap::new();
    for (path, raw) in entries {
        validate_relative_member(path)?;
        let name = file_name(path);
        if !ARCHIVE_RECEIPT_LIMITS
            .iter()
            .any(|(candidate, _)| *candidate == name)
        {
            continue;
        }
        if selected.insert(name.to_owned(), raw.clone()).is_some() {
            return Err(format!("全栈归档包含重复 {name}").into());
        }
    }
    if selected.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected {
        return Err("全栈归档缺少必需的唯一来源、构建或运行收据".into());
    }
    Ok(selected)
}

pub(super) fn validate_core_source(
    source: &Value,
    backend_root: &str,
    backend_sha: &str,
) -> Result<Value> {
    exact_object(
        source,
        &[
            "format_version",
            "fixture",
            "backend_root",
            "source_pair",
            "original",
        ],
        "core 构建来源",
    )?;
    if source.get("format_version").and_then(Value::as_u64) != Some(1)
        || source.get("fixture").and_then(Value::as_str) != Some("core")
        || source.get("backend_root").and_then(Value::as_str) != Some(backend_root)
    {
        return Err("core 构建来源格式无效".into());
    }
    let original = source
        .get("original")
        .ok_or_else(|| "core 原始来源缺失".to_string())?;
    exact_object(original, &["backend"], "core 原始来源")?;
    clean_snapshot(&original["backend"], "core 后端原始来源", backend_sha)?;
    Ok(source.clone())
}

pub(super) fn validate_business_source(
    source: &Value,
    fixture_receipt: &Value,
    fixture_raw: &[u8],
    backend_root: &str,
    backend_sha: &str,
    frontend_sha: &str,
    fixture_sha256: Option<&str>,
) -> Result<Value> {
    exact_object(
        source,
        &[
            "format_version",
            "fixture",
            "fixture_receipt",
            "fixture_definition",
            "roots",
            "source_pair",
            "original",
            "generated",
        ],
        "业务 crate 构建来源",
    )?;
    let fixture_hash = fixture_sha256
        .filter(|value| sha256_shape(Some(value)) && *value != EMPTY_SHA256)
        .ok_or_else(|| "发布源码 业务 crate fixture 摘要无效".to_string())?;
    if source.get("format_version").and_then(Value::as_u64) != Some(1)
        || source.get("fixture").and_then(Value::as_str) != Some("business")
    {
        return Err("业务 crate 构建来源格式无效".into());
    }
    let fixture_binding = validate_binding(
        &source["fixture_receipt"],
        "业务 crate fixture 收据绑定",
        Some(FIXTURE_RECEIPT),
        Some(fixture_raw),
    )?;
    let fixture_root = parent(&fixture_binding.path);
    let roots = &source["roots"];
    exact_object(roots, &["backend", "frontend"], "业务 crate 工作树根目录")?;
    let backend = roots["backend"]
        .as_str()
        .ok_or_else(|| "业务 crate 后端工作树无效".to_string())?;
    let frontend = roots["frontend"]
        .as_str()
        .ok_or_else(|| "业务 crate 前端工作树无效".to_string())?;
    validate_absolute_posix(backend, "业务 crate backend 工作树")?;
    validate_absolute_posix(frontend, "业务 crate frontend 工作树")?;
    if backend != backend_root
        || backend != join_posix(&fixture_root, "backend")
        || frontend != join_posix(&fixture_root, "frontend")
    {
        return Err("业务 crate 工作树与 fixture 收据路径不匹配".into());
    }
    let original = &source["original"];
    let generated = &source["generated"];
    exact_object(original, &["backend", "frontend"], "业务 crate 原始来源")?;
    exact_object(generated, &["backend", "frontend"], "业务 crate 生成来源")?;
    clean_snapshot(
        &original["backend"],
        "业务 crate 原始 backend 来源",
        backend_sha,
    )?;
    generated_snapshot(
        &generated["backend"],
        "业务 crate 生成 backend 来源",
        backend_sha,
    )?;
    clean_snapshot(
        &original["frontend"],
        "业务 crate 原始 frontend 来源",
        frontend_sha,
    )?;
    generated_snapshot(
        &generated["frontend"],
        "业务 crate 生成 frontend 来源",
        frontend_sha,
    )?;
    exact_object(
        fixture_receipt,
        &[
            "format_version",
            "fixture",
            "status",
            "fixture_sha256",
            "sources",
            "paths",
            "generated",
        ],
        "归档业务 crate fixture 收据",
    )?;
    if fixture_receipt
        .get("format_version")
        .and_then(Value::as_u64)
        != Some(1)
        || fixture_receipt.get("fixture").and_then(Value::as_str) != Some("business")
        || fixture_receipt.get("status").and_then(Value::as_str) != Some("ready")
        || fixture_receipt
            .get("fixture_sha256")
            .and_then(Value::as_str)
            != Some(fixture_hash)
        || fixture_receipt.get("sources") != Some(original)
        || fixture_receipt.get("paths") != Some(roots)
        || fixture_receipt.get("generated") != Some(generated)
    {
        return Err("归档业务 crate fixture 与构建来源或发布源码不匹配".into());
    }
    let definition = &source["fixture_definition"];
    exact_object(definition, &["model"], "业务 crate fixture 定义绑定")?;
    let expected_path = join_posix(backend_root, BUSINESS_MODEL);
    let model = validate_binding(&definition["model"], "业务 crate model 定义", None, None)?;
    if model.path != expected_path || model.sha256 != fixture_hash {
        return Err("业务 crate fixture 定义路径或摘要不匹配".into());
    }
    let files = generated["backend"]
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| "业务 crate 生成文件列表无效".to_string())?;
    let model_digest = files.iter().find_map(|entry| {
        (entry.get("path").and_then(Value::as_str)
            == Some("crates/order-business/src/resources/mod.rs"))
        .then(|| entry.get("sha256").and_then(Value::as_str))
        .flatten()
    });
    if model_digest != Some(fixture_hash) {
        return Err("业务 crate 生成快照未绑定当前模型".into());
    }
    Ok(source.clone())
}
