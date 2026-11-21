use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::Result;

use super::{
    evidence::EMPTY_SHA256,
    support::{
        commit_shape, exact_object, parse_json, positive_integer, sha256, sha256_shape,
        string_field,
    },
};

#[path = "archive/frontend.rs"]
mod frontend;
#[path = "archive/input.rs"]
mod input;
#[path = "archive/reader.rs"]
mod reader;
#[path = "archive/snapshot.rs"]
mod snapshot;

pub(crate) use reader::read_archive;
pub(crate) use snapshot::{source_pair_receipt, verify_source_pair_receipt};

pub(crate) const SOURCE_PAIR_RECEIPT: &str = "source-pair.json";
pub(crate) const BUILD_EVIDENCE: &str = "build-evidence.json";
pub(crate) const RUNTIME_RECEIPT: &str = "runtime.json";
pub(crate) const RUNTIME_EVIDENCE: &str = "runtime-evidence.json";
pub(crate) const FIXTURE_RECEIPT: &str = "fixture.json";
pub(crate) const FRONTEND_SOURCE: &str = "frontend-source.json";
pub(crate) const FRONTEND_BUILD: &str = "frontend-build.json";
pub(crate) const BINARY_ROLES: &[&str] = &[
    "ryframe",
    "ryframe-worker",
    "ryframe-reset",
    "ryframe-migrate",
];
pub(crate) const BUSINESS_MODEL: &str = "tools/python/fixtures/order-business/src/resources/mod.rs";
pub(crate) const ARCHIVE_RECEIPT_LIMITS: &[(&str, usize)] = &[
    (SOURCE_PAIR_RECEIPT, 16 * 1024),
    (BUILD_EVIDENCE, 4 * 1024 * 1024),
    (RUNTIME_RECEIPT, 64 * 1024),
    (RUNTIME_EVIDENCE, 4 * 1024 * 1024),
    (FIXTURE_RECEIPT, 4 * 1024 * 1024),
    (FRONTEND_SOURCE, 16 * 1024 * 1024),
    (FRONTEND_BUILD, 16 * 1024 * 1024),
];

const PAIR_FIELDS: &[&str] = &[
    "format_version",
    "backend_sha",
    "frontend_sha",
    "run_id",
    "attempt",
    "sources",
];

pub(crate) fn validate_archive_evidence(
    entries: &[(String, Vec<u8>)],
    fixture: &str,
    backend_sha: &str,
    frontend_sha: &str,
    run_id: u64,
    attempt: u64,
    fixture_sha256: Option<&str>,
) -> Result<Value> {
    if !commit_shape(Some(backend_sha)) || !commit_shape(Some(frontend_sha)) {
        return Err("全栈归档目标提交 SHA 无效".into());
    }
    if run_id == 0 || attempt == 0 {
        return Err("全栈归档 CI 运行身份无效".into());
    }
    let members = input::archive_members(entries, fixture)?;
    let receipts = members
        .iter()
        .map(|(name, raw)| {
            let limit = ARCHIVE_RECEIPT_LIMITS
                .iter()
                .find_map(|(candidate, limit)| (*candidate == name.as_str()).then_some(*limit))
                .ok_or_else(|| format!("未知全栈收据 {name}"))?;
            Ok((
                name.clone(),
                parse_json(raw, &format!("归档 {name}"), limit)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let build = receipts
        .get(BUILD_EVIDENCE)
        .ok_or_else(|| "全栈归档缺少构建证据".to_string())?;
    let backend_root = build_header(build)?;
    let artifacts = build
        .get("artifacts")
        .ok_or_else(|| "归档全栈构建产物缺失".to_string())?;
    exact_object(artifacts, BINARY_ROLES, "归档全栈构建产物")?;
    let mut artifact_paths = BTreeSet::new();
    for role in BINARY_ROLES {
        let binding = validate_binding(
            &artifacts[role],
            &format!("归档构建产物 {role}"),
            None,
            None,
        )?;
        if file_name(&binding.path) != *role || !artifact_paths.insert(binding.path.clone()) {
            return Err("归档构建产物角色、路径或唯一性无效".into());
        }
    }
    let source = if fixture == "core" {
        input::validate_core_source(
            build
                .get("source")
                .ok_or_else(|| "core 构建来源缺失".to_string())?,
            backend_root,
            backend_sha,
        )?
    } else {
        let fixture_raw = members
            .get(FIXTURE_RECEIPT)
            .ok_or_else(|| "业务 crate fixture 收据缺失".to_string())?;
        input::validate_business_source(
            build
                .get("source")
                .ok_or_else(|| "业务 crate 构建来源缺失".to_string())?,
            receipts
                .get(FIXTURE_RECEIPT)
                .ok_or_else(|| "业务 crate fixture 收据缺失".to_string())?,
            fixture_raw,
            backend_root,
            backend_sha,
            frontend_sha,
            fixture_sha256,
        )?
    };
    let pair = receipts
        .get(SOURCE_PAIR_RECEIPT)
        .ok_or_else(|| "全栈源码组合收据缺失".to_string())?;
    let pair_raw = members
        .get(SOURCE_PAIR_RECEIPT)
        .ok_or_else(|| "全栈源码组合收据缺失".to_string())?;
    let receipt_root = validate_archive_pair(
        &source,
        pair,
        pair_raw,
        backend_sha,
        frontend_sha,
        run_id,
        attempt,
    )?;
    let runtime = validate_runtime(
        receipts
            .get(RUNTIME_RECEIPT)
            .ok_or_else(|| "归档运行收据缺失".to_string())?,
        backend_root,
        run_id,
        attempt,
        artifacts,
    )?;
    let runtime_evidence = receipts
        .get(RUNTIME_EVIDENCE)
        .ok_or_else(|| "归档运行来源证据缺失".to_string())?;
    validate_runtime_evidence(
        runtime_evidence,
        &source,
        &receipt_root,
        build,
        &runtime,
        members
            .get(BUILD_EVIDENCE)
            .ok_or_else(|| "全栈构建证据缺失".to_string())?,
        members
            .get(RUNTIME_RECEIPT)
            .ok_or_else(|| "归档运行收据缺失".to_string())?,
    )?;
    let mut result = json!({
        "source_pair": pair,
        "build_evidence": build,
        "runtime": runtime,
        "runtime_evidence": runtime_evidence,
    });
    if fixture == "business" {
        result["fixture_receipt"] = receipts[FIXTURE_RECEIPT].clone();
        result["frontend_source"] = frontend::validate(
            &receipts,
            &members[FRONTEND_BUILD],
            &source,
            backend_root,
            frontend_sha,
            &receipt_root,
        )?;
    }
    Ok(result)
}

fn build_header(build: &Value) -> Result<&str> {
    exact_object(
        build,
        &[
            "format_version",
            "kind",
            "backend_root",
            "source",
            "artifacts",
            "build_jobs",
        ],
        "归档全栈构建证据",
    )?;
    if build.get("format_version").and_then(Value::as_u64) != Some(1)
        || build.get("kind").and_then(Value::as_str) != Some("full-stack-build")
        || build
            .get("build_jobs")
            .and_then(Value::as_u64)
            .is_none_or(|jobs| !(1..=64).contains(&jobs))
    {
        return Err("归档全栈构建证据格式无效".into());
    }
    let backend = build
        .get("backend_root")
        .and_then(Value::as_str)
        .ok_or_else(|| "归档构建后端根目录缺失".to_string())?;
    validate_absolute_posix(backend, "归档构建后端根目录")?;
    Ok(backend)
}

fn validate_archive_pair(
    source: &Value,
    pair: &Value,
    pair_raw: &[u8],
    backend_sha: &str,
    frontend_sha: &str,
    run_id: u64,
    attempt: u64,
) -> Result<String> {
    exact_object(pair, PAIR_FIELDS, "归档源码组合收据")?;
    let sources = &pair["sources"];
    exact_object(sources, &["backend", "frontend"], "归档源码组合来源")?;
    clean_snapshot(&sources["backend"], "归档 backend 原始来源", backend_sha)?;
    clean_snapshot(&sources["frontend"], "归档 frontend 原始来源", frontend_sha)?;
    if pair.get("format_version").and_then(Value::as_u64) != Some(1)
        || pair.get("backend_sha").and_then(Value::as_str) != Some(backend_sha)
        || pair.get("frontend_sha").and_then(Value::as_str) != Some(frontend_sha)
        || pair.get("run_id").and_then(Value::as_u64) != Some(run_id)
        || pair.get("attempt").and_then(Value::as_u64) != Some(attempt)
    {
        return Err("归档源码组合与目标提交或 CI attempt 不匹配".into());
    }
    let embedded = &source["source_pair"];
    let mut embedded_fields = PAIR_FIELDS.to_vec();
    embedded_fields.push("receipt");
    exact_object(embedded, &embedded_fields, "构建来源中的源码组合")?;
    if PAIR_FIELDS
        .iter()
        .any(|field| embedded.get(*field) != pair.get(*field))
    {
        return Err("构建来源与归档源码组合收据不匹配".into());
    }
    let expected_original = if source.get("fixture").and_then(Value::as_str) == Some("business") {
        sources.clone()
    } else {
        json!({"backend": sources["backend"]})
    };
    if source.get("original") != Some(&expected_original) {
        return Err("构建来源与源码组合中的干净来源不匹配".into());
    }
    let binding = validate_binding(
        &embedded["receipt"],
        "源码组合收据绑定",
        Some(SOURCE_PAIR_RECEIPT),
        Some(pair_raw),
    )?;
    Ok(parent(&binding.path))
}

fn validate_runtime(
    runtime: &Value,
    backend_root: &str,
    run_id: u64,
    attempt: u64,
    build_artifacts: &Value,
) -> Result<Value> {
    exact_object(
        runtime,
        &[
            "format_version",
            "backend_root",
            "scope_id",
            "configuration_sha256",
            "worker_ready_url",
            "artifacts",
        ],
        "归档运行收据",
    )?;
    let scope = format!("ci-{run_id}-{attempt}");
    if runtime.get("format_version").and_then(Value::as_u64) != Some(1)
        || runtime.get("backend_root").and_then(Value::as_str) != Some(backend_root)
        || runtime.get("scope_id").and_then(Value::as_str) != Some(scope.as_str())
    {
        return Err("归档运行收据格式、后端根目录或 scope 无效".into());
    }
    if !sha256_shape(runtime.get("configuration_sha256").and_then(Value::as_str)) {
        return Err("归档运行配置摘要无效".into());
    }
    let ready_url = runtime
        .get("worker_ready_url")
        .and_then(Value::as_str)
        .ok_or_else(|| "归档 Worker 就绪地址无效".to_string())?;
    let pattern = Regex::new(r"^http://127\.0\.0\.1:([1-9][0-9]{0,4})/readyz$")
        .map_err(|error| format!("Worker 地址规则无效：{error}"))?;
    pattern
        .captures(ready_url)
        .and_then(|captures| captures.get(1))
        .and_then(|port| port.as_str().parse::<u16>().ok())
        .filter(|port| (1..=65535).contains(port))
        .ok_or_else(|| "归档 Worker 就绪地址无效".to_string())?;
    let artifacts = &runtime["artifacts"];
    exact_object(artifacts, &["api", "worker"], "归档运行产物")?;
    for (runtime_role, build_role) in [("api", "ryframe"), ("worker", "ryframe-worker")] {
        let value = exact_object(
            &artifacts[runtime_role],
            &["path", "sha256"],
            &format!("归档 {runtime_role} 产物"),
        )?;
        let path = string_field(value, "path", "归档运行产物")?;
        validate_absolute_posix(path, "归档运行产物")?;
        let digest = string_field(value, "sha256", "归档运行产物")?;
        if !sha256_shape(Some(digest))
            || path != build_artifacts[build_role]["path"]
            || digest != build_artifacts[build_role]["sha256"]
        {
            return Err("归档运行产物与构建角色的路径或摘要不匹配".into());
        }
    }
    Ok(runtime.clone())
}

fn validate_runtime_evidence(
    runtime_evidence: &Value,
    source: &Value,
    receipt_root: &str,
    build: &Value,
    runtime: &Value,
    build_raw: &[u8],
    runtime_raw: &[u8],
) -> Result<()> {
    exact_object(
        runtime_evidence,
        &[
            "format_version",
            "kind",
            "source",
            "build_evidence",
            "runtime",
        ],
        "归档全栈运行来源证据",
    )?;
    if runtime_evidence
        .get("format_version")
        .and_then(Value::as_u64)
        != Some(1)
        || runtime_evidence.get("kind").and_then(Value::as_str) != Some("full-stack-runtime")
        || runtime_evidence.get("source") != Some(source)
    {
        return Err("归档运行来源证据格式或源码来源不匹配".into());
    }
    let build_binding = validate_binding(
        &runtime_evidence["build_evidence"],
        "全栈构建证据收据绑定",
        Some(BUILD_EVIDENCE),
        Some(build_raw),
    )?;
    let runtime_binding = validate_binding(
        &runtime_evidence["runtime"],
        "全栈运行收据绑定",
        Some(RUNTIME_RECEIPT),
        Some(runtime_raw),
    )?;
    if parent(&build_binding.path) != receipt_root || parent(&runtime_binding.path) != receipt_root
    {
        return Err("源码、构建和运行收据不属于同一全栈运行目录".into());
    }
    let _ = (build, runtime);
    Ok(())
}

struct Binding {
    path: String,
    sha256: String,
}

fn validate_binding(
    value: &Value,
    label: &str,
    filename: Option<&str>,
    raw: Option<&[u8]>,
) -> Result<Binding> {
    let object = exact_object(value, &["path", "bytes", "sha256"], label)?;
    let path = string_field(object, "path", label)?.to_owned();
    validate_absolute_posix(&path, label)?;
    if filename.is_some_and(|filename| file_name(&path) != filename) {
        return Err(format!("{label}文件名无效").into());
    }
    let bytes = positive_integer(object.get("bytes"), &format!("{label}字节数"))?;
    let digest = string_field(object, "sha256", label)?;
    if !sha256_shape(Some(digest)) {
        return Err(format!("{label}摘要无效").into());
    }
    if let Some(raw) = raw
        && (bytes != raw.len() as u64 || digest != sha256(raw))
    {
        return Err(format!("{label}与归档中的原始字节不匹配").into());
    }
    Ok(Binding {
        path,
        sha256: digest.to_owned(),
    })
}

fn snapshot(value: &Value, label: &str) -> Result<Map<String, Value>> {
    let object = exact_object(value, &["head", "patch_sha256", "files"], label)?;
    let head = string_field(object, "head", label)?;
    if !commit_shape(Some(head)) {
        return Err(format!("{label} HEAD 不是有效提交 SHA").into());
    }
    if !sha256_shape(object.get("patch_sha256").and_then(Value::as_str)) {
        return Err(format!("{label}补丁摘要无效").into());
    }
    let files = object
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{label}未跟踪文件清单无效"))?;
    let mut previous = "";
    for entry in files {
        let item = exact_object(entry, &["path", "sha256"], &format!("{label}未跟踪文件"))?;
        let path = string_field(item, "path", label)?;
        validate_relative_member(path)?;
        if path <= previous {
            return Err(format!("{label}未跟踪文件必须无重复并按路径排序").into());
        }
        previous = path;
        if !sha256_shape(item.get("sha256").and_then(Value::as_str)) {
            return Err(format!("{label}未跟踪文件摘要无效").into());
        }
    }
    Ok(object.clone())
}

fn clean_snapshot(value: &Value, label: &str, expected_head: &str) -> Result<()> {
    let receipt = snapshot(value, label)?;
    if receipt.get("head").and_then(Value::as_str) != Some(expected_head)
        || receipt.get("patch_sha256").and_then(Value::as_str) != Some(EMPTY_SHA256)
        || !receipt
            .get("files")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
    {
        return Err(format!("{label}必须是目标提交的干净源码").into());
    }
    Ok(())
}

fn generated_snapshot(value: &Value, label: &str, expected_head: &str) -> Result<()> {
    let receipt = snapshot(value, label)?;
    if receipt.get("head").and_then(Value::as_str) != Some(expected_head)
        || (receipt.get("patch_sha256").and_then(Value::as_str) == Some(EMPTY_SHA256)
            && receipt
                .get("files")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty))
    {
        return Err(format!("{label}没有绑定目标提交上的实际生成变化").into());
    }
    Ok(())
}

fn validate_absolute_posix(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.contains(['\\', '\0'])
        || !value.starts_with('/')
        || value.starts_with("//")
        || (value.len() > 1 && value.ends_with('/'))
        || value[1..]
            .split('/')
            .any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(format!("{label}必须是规范绝对 POSIX 路径").into());
    }
    Ok(())
}

fn validate_relative_member(value: &str) -> Result<()> {
    if value.is_empty()
        || value.contains(['\\', '\0'])
        || value.starts_with('/')
        || value.ends_with('/')
        || value.split('/').any(|part| matches!(part, "" | "." | ".."))
        || value
            .split('/')
            .next()
            .is_some_and(|part| part.len() == 2 && part.ends_with(':'))
    {
        return Err("全栈归档成员路径无效".into());
    }
    Ok(())
}

fn file_name(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or_default()
}

fn parent(value: &str) -> String {
    match value.rsplit_once('/') {
        Some(("", _)) => "/".to_owned(),
        Some((parent, _)) => parent.to_owned(),
        None => String::new(),
    }
}

fn join_posix(parent: &str, child: &str) -> String {
    if parent == "/" {
        format!("/{child}")
    } else {
        format!("{parent}/{child}")
    }
}
