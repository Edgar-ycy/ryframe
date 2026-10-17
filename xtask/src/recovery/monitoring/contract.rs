use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use chrono::DateTime;
use regex::Regex;
use serde_json::{Map, Value};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{digest, json},
};

pub(crate) const TOKEN_NAME: &str = "metrics-token.txt";
pub(crate) const STAGING_DIRECTORY: &str = "input-staging";
pub(crate) const STAGING_MANIFEST: &str = "manifest.json";
pub(crate) const STAGED_RUNNER: &str = "tools/python/restore_monitoring_delivery.py";
pub(crate) const BASE_NAMES: [&str; 3] = ["binding.json", TOKEN_NAME, STAGING_DIRECTORY];
pub(crate) const RUNTIME_DIRECTORIES: [&str; 5] = [
    "configs",
    "processes",
    "evidence",
    "prometheus-data",
    "alertmanager-data",
];
pub(crate) const RECEIPT_NAMES: [&str; 9] = [
    "start-intent.json",
    "start.json",
    "start-failure.json",
    "observation.json",
    "observation-failure.json",
    "close-intent.json",
    "close.json",
    "close-failure.json",
    "result.json",
];
pub(crate) const ROLES: [&str; 3] = ["webhook", "alertmanager", "prometheus"];
pub(crate) const CONFIG_NAMES: [(&str, &str); 4] = [
    ("alertmanager", "alertmanager.yml"),
    ("prometheus", "prometheus.yml"),
    ("rules", "ryframe-alerts.yml"),
    ("rule-tests", "ryframe-alerts.test.yml"),
];
pub(crate) const VALIDATION_NAMES: [(&str, &str); 4] = [
    ("alertmanager", "validate-alertmanager.log"),
    ("prometheus", "validate-prometheus.log"),
    ("rules", "validate-rules.log"),
    ("boundaries", "validate-boundaries.log"),
];
pub(crate) const OBSERVATION_FILES: [&str; 6] = [
    "api-metrics.txt",
    "worker-metrics.txt",
    "prometheus-targets.json",
    "prometheus-rules.json",
    "promtool-boundaries.log",
    "webhook-events.jsonl",
];

#[derive(Debug, Clone)]
pub(crate) struct Document {
    pub(crate) path: PathBuf,
    pub(crate) raw: Vec<u8>,
    pub(crate) value: Value,
    pub(crate) sha256: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Binding {
    pub(crate) document: Document,
    pub(crate) value: Value,
    pub(crate) run: PathBuf,
}

pub(crate) fn read_binding(backend: &Path, path: &Path) -> Result<Binding> {
    validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("监控绑定无效：{error}"))?;
    if path.file_name().and_then(|value| value.to_str()) != Some("binding.json") {
        return Err("监控绑定必须使用绝对 binding.json".into());
    }
    let run = path.parent().ok_or("监控绑定缺少运行目录")?.to_path_buf();
    require_base_members(&run)?;
    let document = read_document(path, "监控绑定")?;
    let value = validate_binding(&document.value)?;
    require_base_members(&run)?;
    validate_root_shape(&run)?;
    document.assert_unchanged()?;
    Ok(Binding {
        document,
        value,
        run,
    })
}

pub(crate) fn read_document(path: &Path, label: &str) -> Result<Document> {
    let before = digest::file(path)?;
    let raw = fs::read(path)?;
    if raw.len() as u64 != before.bytes {
        return Err(format!("{label}在读取期间发生变化").into());
    }
    let text = std::str::from_utf8(&raw).map_err(|_| format!("{label}必须是 UTF-8 JSON"))?;
    let value = json::parse_fragment(text, label)?;
    if !value.is_object() {
        return Err(format!("{label}必须是 JSON 对象").into());
    }
    if digest::file(path)? != before {
        return Err(format!("{label}在摘要采集期间发生变化").into());
    }
    Ok(Document {
        path: path.to_path_buf(),
        raw,
        value,
        sha256: before.sha256,
    })
}

impl Document {
    pub(crate) fn descriptor(&self) -> Value {
        descriptor(&self.path, self.raw.len() as u64, self.sha256.clone())
    }

    pub(crate) fn assert_unchanged(&self) -> Result<()> {
        let current = digest::file(&self.path)?;
        if current.bytes != self.raw.len() as u64 || current.sha256 != self.sha256 {
            return Err(format!("核验期间 JSON 输入被替换或修改：{}", self.path.display()).into());
        }
        Ok(())
    }
}

pub(crate) fn exact<'a>(
    value: &'a Value,
    fields: &[&str],
    label: &str,
) -> Result<&'a Map<String, Value>> {
    json::exact(value, fields, label)
}

pub(crate) fn descriptor(path: &Path, bytes: u64, sha256: String) -> Value {
    serde_json::json!({
        "path": path.to_string_lossy(),
        "bytes": bytes,
        "sha256": sha256,
    })
}

pub(crate) fn artifact(value: &Value, label: &str, expected: Option<&Path>) -> Result<Value> {
    let object = exact(value, &["path", "bytes", "sha256"], label)?;
    let path = absolute_text(object.get("path"), label)?;
    let bytes = object
        .get("bytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{label}字节数无效"))?;
    let sha256 = hex_text(object.get("sha256"), 64, label)?;
    if expected.is_some_and(|expected| path != expected) {
        return Err(format!("{label}不属于当前监控 run").into());
    }
    let actual = digest::file(&path)?;
    if actual.bytes != bytes || actual.sha256 != sha256 {
        return Err(format!("{label}在发布后发生变化").into());
    }
    Ok(value.clone())
}

fn require_base_members(directory: &Path) -> Result<()> {
    require_directory(directory)?;
    for (name, wants_directory) in [
        ("binding.json", false),
        (TOKEN_NAME, false),
        (STAGING_DIRECTORY, true),
    ] {
        let metadata = fs::symlink_metadata(directory.join(name))
            .map_err(|_| format!("监控 run 缺少固定成员 {name}"))?;
        if digest::linked(&metadata)
            || (wants_directory && !metadata.is_dir())
            || (!wants_directory && !metadata.is_file())
        {
            return Err(format!("监控 run 固定成员 {name} 类型无效").into());
        }
    }
    Ok(())
}

pub(crate) fn validate_root_shape(run_directory: &Path) -> Result<BTreeSet<String>> {
    require_directory(run_directory)?;
    let entries = fs::read_dir(run_directory)?.collect::<std::result::Result<Vec<_>, _>>()?;
    let observed = entries
        .iter()
        .map(|entry| {
            entry
                .file_name()
                .into_string()
                .map_err(|_| "监控 run 文件名必须是 UTF-8".to_owned())
        })
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    if observed.iter().any(|name| pending_name(name)) {
        return Err("监控最终成功收据存在待核对 pending，发布状态不明".into());
    }
    let allowed = BASE_NAMES
        .iter()
        .chain(RUNTIME_DIRECTORIES.iter())
        .chain(RECEIPT_NAMES.iter())
        .copied()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if !observed.is_subset(&allowed) || !BASE_NAMES.iter().all(|name| observed.contains(*name)) {
        return Err("监控 run 包含未知写入或缺少固定绑定".into());
    }
    for entry in entries {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "监控 run 文件名必须是 UTF-8")?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if RUNTIME_DIRECTORIES.contains(&name.as_str()) || name == STAGING_DIRECTORY {
            if digest::linked(&metadata) || !metadata.is_dir() {
                return Err("监控 run 的登记写入路径不是普通目录".into());
            }
        } else if digest::linked(&metadata) || !metadata.is_file() {
            return Err("监控 run 根成员不是普通文件".into());
        }
    }
    for (success, failure) in [
        ("start.json", "start-failure.json"),
        ("observation.json", "observation-failure.json"),
        ("close.json", "close-failure.json"),
    ] {
        if observed.contains(success) && observed.contains(failure) {
            return Err("监控 run 同时存在阶段成功与失败收据".into());
        }
    }
    if RUNTIME_DIRECTORIES
        .iter()
        .any(|name| observed.contains(*name))
        && !observed.contains("start-intent.json")
    {
        return Err("监控 run 写入目录缺少启动意图".into());
    }
    if (observed.contains("close.json") || observed.contains("close-failure.json"))
        && !observed.contains("close-intent.json")
    {
        return Err("监控 run 关闭收据缺少关闭意图".into());
    }
    if observed.contains("result.json")
        && !["start.json", "observation.json", "close.json"]
            .iter()
            .all(|name| observed.contains(*name))
    {
        return Err("监控最终结果缺少成功阶段".into());
    }
    Ok(observed)
}

pub(crate) fn validate_runtime_layout(
    run_directory: &Path,
    operations: &Map<String, Value>,
    observed: bool,
) -> Result<()> {
    for name in RUNTIME_DIRECTORIES {
        let metadata = fs::symlink_metadata(run_directory.join(name))?;
        if digest::linked(&metadata) || !metadata.is_dir() {
            return Err("监控 run 缺少登记写入目录".into());
        }
    }
    let configs = CONFIG_NAMES
        .iter()
        .chain(VALIDATION_NAMES.iter())
        .map(|(_, name)| *name)
        .collect::<Vec<_>>();
    exact_files(
        &run_directory.join("configs"),
        &configs,
        "监控配置目录不是精确固定集合",
    )?;
    let evidence = if observed {
        OBSERVATION_FILES.as_slice()
    } else {
        &["webhook-events.jsonl"]
    };
    exact_files(
        &run_directory.join("evidence"),
        evidence,
        "监控证据目录不是当前阶段的精确集合",
    )?;
    let process_directory = run_directory.join("processes");
    let names = directory_names(&process_directory)?;
    let mut allowed = BTreeSet::new();
    for role in ROLES {
        let operation = operations
            .get(role)
            .and_then(Value::as_str)
            .filter(|value| valid_operation_id(value))
            .ok_or("监控启动操作 ID 无效")?;
        for expected in [
            format!("{role}.log"),
            format!("{role}.json"),
            format!("{role}-tree.json"),
            format!("{role}-members-{operation}-ready.json"),
            format!("{role}-members-{operation}-stopped.json"),
            format!("{role}-tree-{operation}-control.json"),
            format!("{role}-tree-{operation}-result.json"),
        ] {
            allowed.insert(expected.clone());
            if !names.contains(&expected) {
                return Err("监控进程树目录缺少登记成员".into());
            }
        }
    }
    if !names.is_subset(&allowed) {
        return Err("进程树目录包含未登记文件".into());
    }
    Ok(())
}

pub(crate) fn process_identity(value: &Value, label: &str) -> Result<(u32, Value)> {
    let object = exact(value, &["pid", "started", "executable"], label)?;
    let pid = object
        .get("pid")
        .and_then(Value::as_u64)
        .filter(|value| *value > 1)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| format!("{label}缺少有效 PID"))?;
    let started = object
        .get("started")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(|| format!("{label}创建代次无效"))?;
    let executable = absolute_text(object.get("executable"), label)?;
    Ok((
        pid,
        serde_json::json!({
            "pid": pid,
            "started": started,
            "executable": executable.to_string_lossy(),
        }),
    ))
}

pub(crate) fn rule_alerts(binding: &Value) -> Result<BTreeSet<String>> {
    let rules = exact(
        binding.get("rules").ok_or("监控绑定缺少规则")?,
        &["alerts", "tests"],
        "监控规则",
    )?;
    let path = absolute_text(
        Some(
            rules
                .get("alerts")
                .and_then(|value| value.get("path"))
                .ok_or("监控告警规则路径无效")?,
        ),
        "监控告警规则",
    )?;
    let metadata = fs::symlink_metadata(&path)?;
    if digest::linked(&metadata) || !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err("监控告警规则不是可信普通文件".into());
    }
    let text = String::from_utf8(fs::read(path)?).map_err(|_| "监控告警规则不是 UTF-8")?;
    let pattern = Regex::new(r"(?m)^\s*-\s*alert:\s*([A-Za-z][A-Za-z0-9_]*)\s*$")?;
    let alerts = pattern
        .captures_iter(&text)
        .map(|capture| capture[1].to_owned())
        .collect::<BTreeSet<_>>();
    if alerts.is_empty() {
        return Err("监控告警规则没有定义任何告警".into());
    }
    Ok(alerts)
}

pub(crate) use super::binding::validate_binding;

pub(crate) fn receipt_descriptor(value: &Value, label: &str, positive: bool) -> Result<Value> {
    let receipt = exact(value, &["path", "bytes", "sha256"], label)?;
    absolute_text(receipt.get("path"), label)?;
    let bytes = receipt
        .get("bytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{label}不是完整文件描述"))?;
    if positive && bytes == 0 {
        return Err(format!("{label}不是完整文件描述").into());
    }
    hex_text(receipt.get("sha256"), 64, label)?;
    Ok(value.clone())
}

pub(crate) fn timestamp(value: Option<&Value>, label: &str) -> Result<()> {
    let text = value
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{label}必须是带时区的时间字符串"))?;
    DateTime::parse_from_rfc3339(text).map_err(|_| format!("{label}不是有效时间"))?;
    Ok(())
}

pub(crate) fn valid_operation_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn absolute_text(value: Option<&Value>, label: &str) -> Result<PathBuf> {
    let text = value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{label}必须是绝对路径"))?;
    let path = PathBuf::from(text);
    if !path.is_absolute() {
        return Err(format!("{label}必须是绝对路径").into());
    }
    Ok(path)
}

pub(crate) fn hex_text(value: Option<&Value>, length: usize, label: &str) -> Result<String> {
    let text = value
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{label}无效"))?;
    if text.len() != length
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("{label}无效").into());
    }
    Ok(text.to_owned())
}

pub(crate) fn loopback_port(url: &str, path: &str) -> Result<u16> {
    let (host, rest) = if let Some(value) = url.strip_prefix("http://127.0.0.1:") {
        ("127.0.0.1", value)
    } else if let Some(value) = url.strip_prefix("http://[::1]:") {
        ("::1", value)
    } else {
        return Err("监控端点必须是 loopback HTTP".into());
    };
    let split = rest.find('/').unwrap_or(rest.len());
    let port_text = &rest[..split];
    if port_text.is_empty()
        || !port_text.bytes().all(|byte| byte.is_ascii_digit())
        || &rest[split..] != path
    {
        return Err("监控端点必须使用显式 loopback 端口和精确路径".into());
    }
    let port = port_text
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or("监控端点端口无效")?;
    let canonical = if host == "::1" {
        format!("http://[::1]:{port}{path}")
    } else {
        format!("http://127.0.0.1:{port}{path}")
    };
    if url != canonical {
        return Err("监控端点必须使用规范化 loopback 地址".into());
    }
    Ok(port)
}

pub(crate) fn sorted_ports(binding: &Value) -> Vec<String> {
    let mut values = binding["endpoints"]
        .as_object()
        .into_iter()
        .flat_map(|object| object.keys().cloned())
        .collect::<Vec<_>>();
    values.sort();
    values
}

fn require_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if digest::linked(&metadata) || !metadata.is_dir() {
        return Err("监控 run 路径必须是已存在的普通目录".into());
    }
    Ok(())
}

fn directory_names(path: &Path) -> Result<BTreeSet<String>> {
    require_directory(path)?;
    fs::read_dir(path)?
        .map(|entry| {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if digest::linked(&metadata) || !metadata.is_file() {
                return Err("监控运行目录包含链接或非普通文件".into());
            }
            entry
                .file_name()
                .into_string()
                .map_err(|_| "监控运行文件名必须是 UTF-8".into())
        })
        .collect::<Result<BTreeSet<_>>>()
}

fn exact_files(path: &Path, expected: &[&str], error: &str) -> Result<()> {
    let actual = directory_names(path)?;
    let expected = expected.iter().map(|value| (*value).to_owned()).collect();
    if actual != expected {
        return Err(error.into());
    }
    Ok(())
}

fn pending_name(value: &str) -> bool {
    value
        .strip_prefix(".result.json.")
        .and_then(|value| value.strip_suffix(".pending"))
        .is_some_and(valid_operation_id)
}

pub(crate) fn valid_run_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.ends_with('-')
        && !value.contains("--")
}
