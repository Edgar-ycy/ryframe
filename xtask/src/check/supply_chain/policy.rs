use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use chrono::NaiveDate;
use regex::Regex;
use serde_json::{Map, Value};

#[path = "policy/patch.rs"]
mod patch;
#[path = "policy/runtime.rs"]
mod runtime;
use patch::parse_local_patch_policy;
use runtime::parse_runtime_feature_tree_policy;

const TOOL_NAMES: [&str; 5] = [
    "cargo-audit",
    "cargo-deny",
    "cargo-cyclonedx",
    "sccache",
    "trivy",
];
const REQUIRED_FEATURE_TREE_PROFILES: [&str; 4] = ["API", "Worker", "Migrate", "GeneratorDefault"];

pub(crate) type PolicyResult<T> = std::result::Result<T, String>;

#[derive(Debug, Clone)]
pub(crate) struct Policy {
    pub(crate) tools: BTreeMap<String, String>,
    pub(crate) local_patch_licenses: Vec<LocalPatch>,
    pub(crate) service_images: Vec<ServiceImage>,
    pub(crate) vulnerability_gate: VulnerabilityGate,
    pub(crate) runtime_feature_tree_gate: RuntimeFeatureTreeGate,
    pub(crate) dependency_graph_exceptions: Vec<DependencyException>,
}

#[derive(Debug, Clone)]
pub(crate) struct LocalPatch {
    pub(crate) patch: String,
    pub(crate) package: String,
    pub(crate) version: String,
    pub(crate) path: String,
    pub(crate) license_expression: String,
    pub(crate) license_files: Vec<LicenseFile>,
}

#[derive(Debug, Clone)]
pub(crate) struct LicenseFile {
    pub(crate) spdx: String,
    pub(crate) file: String,
    pub(crate) sha256: String,
    pub(crate) registry_source: RegistrySource,
}

#[derive(Debug, Clone)]
pub(crate) enum RegistrySource {
    File {
        package: String,
        version: String,
        file: String,
    },
    ManifestLicense {
        package: String,
        version: String,
        manifest_license: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct ServiceImage {
    pub(crate) workflow: String,
    pub(crate) job: String,
    pub(crate) service: String,
    pub(crate) image: String,
}

#[derive(Debug, Clone)]
pub(crate) struct VulnerabilityGate {
    pub(crate) severities: Vec<String>,
    pub(crate) exceptions: Vec<VulnerabilityException>,
}

#[derive(Debug, Clone)]
pub(crate) struct VulnerabilityException {
    pub(crate) id: String,
    pub(crate) package: String,
    pub(crate) installed_version: String,
    pub(crate) target: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeFeatureTreeGate {
    pub(crate) profiles: Vec<FeatureProfile>,
}

#[derive(Debug, Clone)]
pub(crate) struct FeatureProfile {
    pub(crate) name: String,
    pub(crate) command: FeatureCommand,
    pub(crate) constraints: FeatureConstraints,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FeatureCommand {
    pub(crate) package: String,
    pub(crate) no_default_features: bool,
    pub(crate) features: Vec<String>,
    pub(crate) edges: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct FeatureConstraints {
    pub(crate) required_packages: Vec<String>,
    pub(crate) required_features: BTreeMap<String, Vec<String>>,
    pub(crate) forbidden_packages: Vec<String>,
    pub(crate) forbidden_features: BTreeMap<String, Vec<String>>,
    pub(crate) max_unique_packages: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct DependencyException {
    pub(crate) package: String,
    pub(crate) version: String,
}

pub(crate) fn load_file(path: &Path, today: NaiveDate) -> PolicyResult<Policy> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("无法读取供应链策略 {}: {error}", path.display()))?;
    parse(&source, today)
}

#[allow(dead_code)]
pub(crate) fn parse_vulnerability_gate(source: &str) -> PolicyResult<VulnerabilityGate> {
    let value = serde_json::from_str::<Value>(source)
        .map_err(|error| format!("无法读取供应链策略: {error}"))?;
    let root = object(Some(&value), "供应链策略")?;
    parse_vulnerability_gate_value(
        root.get("vulnerability_gate"),
        chrono::Local::now().date_naive(),
    )
}

pub(crate) fn parse(source: &str, today: NaiveDate) -> PolicyResult<Policy> {
    let value = serde_json::from_str::<Value>(source)
        .map_err(|error| format!("无法读取供应链策略: {error}"))?;
    let policy = object(Some(&value), "供应链策略")?;
    exact_keys(
        policy,
        &[
            "schema_version",
            "tools",
            "vulnerability_gate",
            "dependency_graph_exceptions",
            "service_images",
            "local_patch_licenses",
            "runtime_feature_tree_gate",
        ],
        "供应链策略",
    )?;
    if policy.get("schema_version").and_then(Value::as_u64) != Some(4) {
        return Err("schema_version 只允许为 4".into());
    }

    let tools = parse_tools(policy.get("tools"))?;
    let local_patch_licenses = parse_local_patch_policy(policy.get("local_patch_licenses"))?;
    let runtime_feature_tree_gate =
        parse_runtime_feature_tree_policy(policy.get("runtime_feature_tree_gate"))?;
    let service_images = parse_service_images(policy.get("service_images"))?;
    let vulnerability_gate =
        parse_vulnerability_gate_value(policy.get("vulnerability_gate"), today)?;
    let dependency_graph_exceptions =
        parse_dependency_graph_exceptions(policy.get("dependency_graph_exceptions"), today)?;

    Ok(Policy {
        tools,
        local_patch_licenses,
        service_images,
        vulnerability_gate,
        runtime_feature_tree_gate,
        dependency_graph_exceptions,
    })
}

fn parse_tools(value: Option<&Value>) -> PolicyResult<BTreeMap<String, String>> {
    let tools = object(value, "tools")?;
    exact_keys(tools, &TOOL_NAMES, "tools")?;
    TOOL_NAMES
        .into_iter()
        .map(|name| {
            let version = text(tools.get(name), &format!("工具 {name}"), 1)?;
            if !semver(&version) {
                return Err(format!("工具 {name} 必须固定到完整三段版本"));
            }
            Ok((name.to_owned(), version))
        })
        .collect()
}

fn parse_service_images(value: Option<&Value>) -> PolicyResult<Vec<ServiceImage>> {
    let entries = array(value, "service_images")?;
    let mut seen = BTreeSet::new();
    let mut images = Vec::with_capacity(entries.len());
    for (index, value) in entries.iter().enumerate() {
        let label = format!("服务镜像[{index}]");
        let item = object(Some(value), &label)?;
        exact_keys(item, &["workflow", "job", "service", "image"], &label)?;
        let workflow = text(item.get("workflow"), &format!("{label}.workflow"), 1)?;
        if !workflow_file(&workflow) {
            return Err(format!("{label}.workflow 必须是安全的工作流文件名"));
        }
        let job = text(item.get("job"), &format!("{label}.job"), 1)?;
        let service = text(item.get("service"), &format!("{label}.service"), 1)?;
        let image = text(item.get("image"), &format!("{label}.image"), 1)?;
        let image_name = image.split("@sha256:").next().unwrap_or_default();
        let final_segment = image_name.rsplit('/').next().unwrap_or_default();
        if !service_image(&image) || !final_segment.contains(':') {
            return Err(format!("{label}.image 必须固定到完整 tag 和 sha256 digest"));
        }
        if !seen.insert((workflow.clone(), job.clone(), service.clone())) {
            return Err(format!("{label} 重复声明 {workflow}/{job}/{service}"));
        }
        images.push(ServiceImage {
            workflow,
            job,
            service,
            image,
        });
    }
    Ok(images)
}

fn parse_vulnerability_gate_value(
    value: Option<&Value>,
    today: NaiveDate,
) -> PolicyResult<VulnerabilityGate> {
    let gate = object(value, "vulnerability_gate")?;
    exact_keys(gate, &["severities", "exceptions"], "vulnerability_gate")?;
    let severities = string_array(
        gate.get("severities"),
        "vulnerability_gate.severities",
        false,
    )?;
    let severity_set = severities
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if severities.len() != severity_set.len()
        || severity_set != BTreeSet::from(["HIGH", "CRITICAL"])
    {
        return Err("漏洞门禁必须且只能覆盖 HIGH、CRITICAL".into());
    }
    let entries = array(gate.get("exceptions"), "vulnerability_gate.exceptions")?;
    let mut seen = BTreeSet::new();
    let mut exceptions = Vec::with_capacity(entries.len());
    for (index, value) in entries.iter().enumerate() {
        let label = format!("漏洞例外[{index}]");
        let item = object(Some(value), &label)?;
        let allowed = BTreeSet::from([
            "id",
            "package",
            "installed_version",
            "owner",
            "expires",
            "reason",
            "target",
        ]);
        let required = allowed
            .iter()
            .copied()
            .filter(|key| *key != "target")
            .collect::<BTreeSet<_>>();
        let actual = item.keys().map(String::as_str).collect::<BTreeSet<_>>();
        if !required.is_subset(&actual) || !actual.is_subset(&allowed) {
            return Err(format!("{label} 字段不完整或包含未知字段"));
        }
        let id = text(item.get("id"), &format!("{label}.id"), 1)?;
        if !advisory_id(&id) {
            return Err(format!("{label}.id 格式无效"));
        }
        let package = text(item.get("package"), &format!("{label}.package"), 1)?;
        let installed_version = text(
            item.get("installed_version"),
            &format!("{label}.installed_version"),
            1,
        )?;
        let target = match item.get("target") {
            None | Some(Value::Null) => None,
            Some(_) => Some(text(item.get("target"), &format!("{label}.target"), 1)?),
        };
        text(item.get("owner"), &format!("{label}.owner"), 3)?;
        future_date(item.get("expires"), &format!("{label}.expires"), today)?;
        text(item.get("reason"), &format!("{label}.reason"), 12)?;
        if !seen.insert((
            id.clone(),
            package.clone(),
            installed_version.clone(),
            target.clone(),
        )) {
            return Err(format!("{label} 重复"));
        }
        exceptions.push(VulnerabilityException {
            id,
            package,
            installed_version,
            target,
        });
    }
    Ok(VulnerabilityGate {
        severities,
        exceptions,
    })
}

fn parse_dependency_graph_exceptions(
    value: Option<&Value>,
    today: NaiveDate,
) -> PolicyResult<Vec<DependencyException>> {
    let entries = array(value, "dependency_graph_exceptions")?;
    let mut seen = BTreeSet::new();
    let mut exceptions = Vec::with_capacity(entries.len());
    for (index, value) in entries.iter().enumerate() {
        let label = format!("依赖图例外[{index}]");
        let item = object(Some(value), &label)?;
        exact_keys(
            item,
            &[
                "package",
                "version",
                "enforcement",
                "owner",
                "expires",
                "reason",
            ],
            &label,
        )?;
        let package = text(item.get("package"), &format!("{label}.package"), 1)?;
        let version = text(item.get("version"), &format!("{label}.version"), 1)?;
        if item.get("enforcement").and_then(Value::as_str)
            != Some("must_be_absent_from_resolved_graph")
        {
            return Err(format!("{label} 必须使用失败关闭的执行方式"));
        }
        text(item.get("owner"), &format!("{label}.owner"), 3)?;
        future_date(item.get("expires"), &format!("{label}.expires"), today)?;
        text(item.get("reason"), &format!("{label}.reason"), 12)?;
        if !seen.insert((package.clone(), version.clone())) {
            return Err(format!("{label} 重复"));
        }
        exceptions.push(DependencyException { package, version });
    }
    Ok(exceptions)
}

fn object<'a>(value: Option<&'a Value>, label: &str) -> PolicyResult<&'a Map<String, Value>> {
    value
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{label} 必须是对象"))
}

fn array<'a>(value: Option<&'a Value>, label: &str) -> PolicyResult<&'a [Value]> {
    value
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{label} 必须是数组"))
}

fn exact_keys(value: &Map<String, Value>, expected: &[&str], label: &str) -> PolicyResult<()> {
    let actual = value.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    if actual == expected {
        return Ok(());
    }
    let missing = expected.difference(&actual).copied().collect::<Vec<_>>();
    let extra = actual.difference(&expected).copied().collect::<Vec<_>>();
    Err(format!(
        "{label} 字段不匹配，缺少={missing:?}，多余={extra:?}"
    ))
}

fn text(value: Option<&Value>, label: &str, minimum: usize) -> PolicyResult<String> {
    let Some(value) = value.and_then(Value::as_str) else {
        return Err(format!("{label} 必须是至少 {minimum} 个字符的非空字符串"));
    };
    let trimmed = value.trim();
    if trimmed.chars().count() < minimum {
        return Err(format!("{label} 必须是至少 {minimum} 个字符的非空字符串"));
    }
    Ok(trimmed.to_owned())
}

fn future_date(value: Option<&Value>, label: &str, today: NaiveDate) -> PolicyResult<NaiveDate> {
    let raw = text(value, label, 1)?;
    let expires = NaiveDate::parse_from_str(&raw, "%Y-%m-%d")
        .map_err(|_| format!("{label} 必须是 YYYY-MM-DD 日期"))?;
    if expires <= today {
        return Err(format!("{label} 已到期或不是未来日期：{raw}"));
    }
    Ok(expires)
}

fn string_array(
    value: Option<&Value>,
    label: &str,
    allow_empty: bool,
) -> PolicyResult<Vec<String>> {
    let values = array(value, label)?;
    let mut result = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        result.push(text(Some(value), &format!("{label}[{index}]"), 1)?);
    }
    if !allow_empty && result.is_empty() {
        return Err(format!("{label} 不得为空"));
    }
    Ok(result)
}

fn license_identifiers(value: &str, label: &str) -> PolicyResult<Vec<String>> {
    let identifiers = value.split(" OR ").map(str::to_owned).collect::<Vec<_>>();
    if identifiers.is_empty() || identifiers.iter().any(|item| !spdx_identifier(item)) {
        return Err(format!("{label} 只允许由 SPDX 标识符和 OR 组成"));
    }
    let unique = identifiers.iter().collect::<BTreeSet<_>>();
    if unique.len() != identifiers.len() {
        return Err(format!("{label} 包含重复 SPDX 标识符"));
    }
    Ok(identifiers)
}

fn portable_path(value: Option<&Value>, label: &str) -> PolicyResult<String> {
    let path = text(value, label, 1)?;
    if !portable_path_name(&path)
        || path
            .split('/')
            .any(|component| matches!(component, "." | ".."))
    {
        return Err(format!("{label} 必须是安全的 POSIX 相对路径"));
    }
    Ok(path)
}

fn matches(pattern: &str, value: &str) -> bool {
    Regex::new(pattern)
        .expect("供应链策略固定正则")
        .is_match(value)
}

fn package_name(value: &str) -> bool {
    matches(r"^[A-Za-z0-9][A-Za-z0-9_-]*$", value)
}

fn cargo_feature_name(value: &str) -> bool {
    matches(r"^[A-Za-z0-9_][A-Za-z0-9_.+-]*$", value)
}

fn semver(value: &str) -> bool {
    matches(r"^[0-9]+\.[0-9]+\.[0-9]+$", value)
}

fn lower_sha256(value: &str) -> bool {
    matches(r"^[0-9a-f]{64}$", value)
}

fn spdx_identifier(value: &str) -> bool {
    matches(r"^[A-Za-z0-9][A-Za-z0-9.+-]*$", value)
}

fn portable_path_name(value: &str) -> bool {
    matches(r"^[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*$", value)
}

fn workflow_file(value: &str) -> bool {
    matches(r"^[A-Za-z0-9_.-]+\.ya?ml$", value)
        && Path::new(value)
            .file_name()
            .is_some_and(|name| name == value)
}

fn service_image(value: &str) -> bool {
    matches(r"^[A-Za-z0-9][A-Za-z0-9._:/-]*@sha256:[0-9a-f]{64}$", value)
}

fn advisory_id(value: &str) -> bool {
    matches(r"^[A-Z0-9][A-Z0-9._:-]+$", value)
}
