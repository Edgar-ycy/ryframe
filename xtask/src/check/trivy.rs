//! 单个 Trivy 报告的只读校验，不执行 source 供应链检查或外部工具。

use std::{collections::BTreeSet, fs, path::Path};

use chrono::{Local, NaiveDate};
use serde::Deserialize;
use serde_json::Value;

use crate::{Result, process::PreservedFailure};

const POLICY_PATH: &str = "tools/policies/supply_chain_policy.json";

#[derive(Debug, Deserialize)]
pub(crate) struct Policy {
    vulnerability_gate: VulnerabilityGate,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VulnerabilityGate {
    severities: Vec<String>,
    exceptions: Vec<VulnerabilityException>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VulnerabilityException {
    id: String,
    package: String,
    installed_version: String,
    owner: String,
    expires: String,
    reason: String,
    target: Option<String>,
}

impl VulnerabilityException {
    fn matches(&self, id: &str, package: &str, installed: &str, target: Option<&str>) -> bool {
        self.id == id
            && self.package == package
            && self.installed_version == installed
            && self
                .target
                .as_deref()
                .is_none_or(|expected| target == Some(expected))
    }
}

pub(crate) fn parse_policy(source: &str) -> Result<Policy> {
    let policy: Policy = serde_json::from_str(source).map_err(|_| {
        PreservedFailure::new(
            "无法读取供应链策略：vulnerability_gate 结构无效".to_owned(),
            Some(1),
        )
    })?;
    if !policy.vulnerability_gate.is_valid() {
        return Err(PreservedFailure::new(
            "无法读取供应链策略：vulnerability_gate 结构或取值无效".to_owned(),
            Some(1),
        )
        .into());
    }
    Ok(policy)
}

impl VulnerabilityGate {
    fn is_valid(&self) -> bool {
        let severities = self
            .severities
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if severities.len() != self.severities.len()
            || severities.len() != 2
            || !severities.contains("HIGH")
            || !severities.contains("CRITICAL")
        {
            return false;
        }

        let today = Local::now().date_naive();
        let mut identities = BTreeSet::new();
        self.exceptions.iter().all(|exception| {
            valid_advisory_id(&exception.id)
                && !exception.package.trim().is_empty()
                && !exception.installed_version.trim().is_empty()
                && exception.owner.trim().chars().count() >= 3
                && exception.reason.trim().chars().count() >= 12
                && exception
                    .target
                    .as_deref()
                    .is_none_or(|target| !target.trim().is_empty())
                && valid_future_expiry(&exception.expires, today)
                && identities.insert((
                    exception.id.as_str(),
                    exception.package.as_str(),
                    exception.installed_version.as_str(),
                    exception.target.as_deref(),
                ))
        })
    }
}

fn valid_advisory_id(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    (first.is_ascii_uppercase() || first.is_ascii_digit())
        && characters.next().is_some_and(valid_advisory_character)
        && characters.all(valid_advisory_character)
}

fn valid_advisory_character(character: char) -> bool {
    character.is_ascii_uppercase()
        || character.is_ascii_digit()
        || matches!(character, '.' | '_' | ':' | '-')
}

fn valid_future_expiry(value: &str, today: NaiveDate) -> bool {
    NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").is_ok_and(|expires| expires > today)
}

pub(crate) fn validate(source: &str, policy: &Policy) -> Vec<String> {
    let document = match serde_json::from_str::<Value>(source) {
        Ok(Value::Object(document)) => document,
        _ => return vec!["无法读取 Trivy 报告：必须是 JSON 对象".to_owned()],
    };
    let Some(results) = document.get("Results").and_then(Value::as_array) else {
        return vec!["Trivy 报告缺少 Results 数组".to_owned()];
    };

    let severities = policy
        .vulnerability_gate
        .severities
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let exceptions = &policy.vulnerability_gate.exceptions;
    let mut used = vec![false; exceptions.len()];
    let mut errors = Vec::new();

    for result in results {
        let Some(result) = result.as_object() else {
            errors.push("Trivy Results 包含非对象条目".to_owned());
            continue;
        };
        let target = result.get("Target");
        let vulnerabilities = match result.get("Vulnerabilities") {
            None | Some(Value::Null) => &[][..],
            Some(Value::Array(items)) => items.as_slice(),
            Some(_) => {
                errors.push(format!(
                    "Trivy 目标 {} 的 Vulnerabilities 不是数组",
                    target_label(target)
                ));
                continue;
            }
        };
        check_vulnerabilities(
            vulnerabilities,
            target,
            &severities,
            exceptions,
            &mut used,
            &mut errors,
        );
    }

    for (index, exception) in exceptions.iter().enumerate() {
        if !used[index] {
            errors.push(format!(
                "漏洞例外未被报告使用，必须删除或校正：{} {}@{}",
                exception.id, exception.package, exception.installed_version
            ));
        }
    }
    errors
}

fn check_vulnerabilities(
    vulnerabilities: &[Value],
    target: Option<&Value>,
    severities: &BTreeSet<&str>,
    exceptions: &[VulnerabilityException],
    used: &mut [bool],
    errors: &mut Vec<String>,
) {
    for vulnerability in vulnerabilities {
        let Some(vulnerability) = vulnerability.as_object() else {
            errors.push(format!(
                "Trivy 目标 {} 包含无效漏洞条目",
                target_label(target)
            ));
            continue;
        };
        let Some(severity) = vulnerability.get("Severity").and_then(Value::as_str) else {
            continue;
        };
        if !severities.contains(severity) {
            continue;
        }
        let fields = (
            vulnerability.get("VulnerabilityID").and_then(Value::as_str),
            vulnerability.get("PkgName").and_then(Value::as_str),
            vulnerability
                .get("InstalledVersion")
                .and_then(Value::as_str),
        );
        let (Some(id), Some(package), Some(installed)) = fields else {
            errors.push(format!(
                "Trivy 目标 {} 的高等级漏洞缺少字符串字段",
                target_label(target)
            ));
            continue;
        };
        let target_text = target.and_then(Value::as_str);
        let matched = exceptions
            .iter()
            .enumerate()
            .find(|(_, exception)| exception.matches(id, package, installed, target_text));
        if let Some((index, _)) = matched {
            used[index] = true;
        } else {
            errors.push(format!(
                "未放行的 {severity} 漏洞：{id} {package}@{installed} target={}",
                target_label(target)
            ));
        }
    }
}

fn target_label(target: Option<&Value>) -> String {
    target
        .map(Value::to_string)
        .unwrap_or_else(|| "null".to_owned())
}

pub(crate) fn check(path: &Path, policy: &Policy) -> Result<Vec<String>> {
    let source = fs::read_to_string(path).map_err(|_| {
        // 文件路径、JSON 片段和底层异常均可能携带凭据，诊断只报告固定原因。
        PreservedFailure::new(
            "无法读取 Trivy 报告：文件不可读取或 UTF-8 无效".to_owned(),
            Some(1),
        )
    })?;
    Ok(validate(&source, policy))
}

pub(crate) fn run(path: &Path, backend_root: &Path) -> Result<()> {
    let policy_source = fs::read_to_string(backend_root.join(POLICY_PATH)).map_err(|_| {
        PreservedFailure::new(
            "无法读取供应链策略：文件不可读取或 UTF-8 无效".to_owned(),
            Some(1),
        )
    })?;
    let policy = parse_policy(&policy_source)?;
    let errors = check(path, &policy)?;
    if !errors.is_empty() {
        return Err(PreservedFailure::new(errors.join("\n"), Some(1)).into());
    }
    println!("Trivy 报告校验通过");
    Ok(())
}
