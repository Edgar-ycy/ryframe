//! 单个 Trivy 报告的只读校验，不执行 source 供应链检查或外部工具。

use std::{collections::BTreeSet, fs, path::Path};

use serde_json::Value;

use super::supply_chain::policy::{
    self as supply_policy, VulnerabilityException, VulnerabilityGate,
};
use crate::{Result, process::PreservedFailure};

const POLICY_PATH: &str = "tools/policies/supply_chain_policy.json";

pub(crate) type Policy = VulnerabilityGate;

#[allow(dead_code)]
pub(crate) fn parse_policy(source: &str) -> Result<Policy> {
    supply_policy::parse_vulnerability_gate(source)
        .map_err(|_| {
            PreservedFailure::new(
                "无法读取供应链策略：vulnerability_gate 结构无效".to_owned(),
                Some(1),
            )
        })
        .map_err(Into::into)
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
        .severities
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let exceptions = &policy.exceptions;
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
        let matched = exceptions.iter().enumerate().find(|(_, exception)| {
            exception_matches(exception, id, package, installed, target_text)
        });
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

fn exception_matches(
    exception: &VulnerabilityException,
    id: &str,
    package: &str,
    installed: &str,
    target: Option<&str>,
) -> bool {
    exception.id == id
        && exception.package == package
        && exception.installed_version == installed
        && exception
            .target
            .as_deref()
            .is_none_or(|expected| target == Some(expected))
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
    let policy = supply_policy::load_file(
        &backend_root.join(POLICY_PATH),
        chrono::Local::now().date_naive(),
    )
    .map_err(|_| {
        PreservedFailure::new("无法读取供应链策略：策略结构或取值无效".to_owned(), Some(1))
    })?;
    let errors = check(path, &policy.vulnerability_gate)?;
    if !errors.is_empty() {
        return Err(PreservedFailure::new(errors.join("\n"), Some(1)).into());
    }
    println!("Trivy 报告校验通过");
    Ok(())
}
