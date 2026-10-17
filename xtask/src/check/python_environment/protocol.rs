use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::requirements::Requirement;
use crate::{Result, process::PreservedFailure};

pub(crate) const ENVIRONMENT_PROBE: &str = r#"
import importlib.metadata as metadata
import json
import os
import platform
import sys

try:
    request = json.loads(sys.stdin.buffer.read().decode("utf-8"))
    names = request["distributions"]
    if not isinstance(names, list) or not all(isinstance(name, str) for name in names):
        raise ValueError("distributions")
    versions = {}
    errors = []
    for name in names:
        try:
            versions[name] = metadata.version(name)
        except metadata.PackageNotFoundError:
            versions[name] = None
        except Exception as error:
            versions[name] = None
            errors.append(f"{name}: {type(error).__name__}")
    response = {
        "executable": os.path.realpath(sys.executable),
        "implementation": platform.python_implementation(),
        "version": [sys.version_info.major, sys.version_info.minor, sys.version_info.micro],
        "distribution_versions": versions,
        "errors": errors,
    }
    sys.stdout.buffer.write(
        json.dumps(response, ensure_ascii=False, sort_keys=True).encode("utf-8")
    )
except Exception as error:
    sys.stderr.write(f"Python environment protocol failed: {type(error).__name__}\n")
    raise SystemExit(2)
"#;

pub(crate) const NATIVE_PROBE_MARKER: &str = "Rust AST native probe passed (rounds=64)";

#[derive(Debug, Serialize)]
pub(crate) struct EnvironmentRequest<'a> {
    minimum_python: [u64; 3],
    distributions: Vec<&'a str>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvironmentReport {
    pub(crate) executable: String,
    pub(crate) implementation: String,
    pub(crate) version: Vec<u64>,
    pub(crate) distribution_versions: BTreeMap<String, Option<String>>,
    pub(crate) errors: Vec<String>,
}

pub(crate) fn encode_request(
    requirements: &[Requirement],
    minimum_python: [u64; 3],
) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&EnvironmentRequest {
        minimum_python,
        distributions: requirements
            .iter()
            .map(|requirement| requirement.name.as_str())
            .collect(),
    })?)
}

pub(crate) fn decode_report(stdout: &[u8]) -> Result<EnvironmentReport> {
    let text = std::str::from_utf8(stdout)
        .map_err(|error| format!("Python 环境协议输出不是 UTF-8：{error}"))?;
    if text.trim().is_empty() {
        return Err("Python 环境协议成功退出但没有输出".into());
    }
    serde_json::from_str(text)
        .map_err(|error| format!("Python 环境协议输出不是有效 JSON：{error}").into())
}

pub(crate) fn validate_environment(
    requirements: &[Requirement],
    minimum_python: [u64; 3],
    report: &EnvironmentReport,
) -> Result<()> {
    let mut errors = report.errors.clone();
    if report.executable.trim().is_empty() {
        errors.push("Python 协议未报告实际解释器路径".into());
    }
    if report.implementation.trim().is_empty() {
        errors.push("Python 协议未报告解释器实现".into());
    }
    let [major, minor, patch] = report.version.as_slice() else {
        errors.push("Python 协议版本必须包含 major.minor.patch".into());
        return failure(errors);
    };
    if (*major, *minor, *patch) < (minimum_python[0], minimum_python[1], minimum_python[2]) {
        errors.push(format!(
            "Python 版本过低：至少需要 {}.{}，实际为 {major}.{minor}.{patch}",
            minimum_python[0], minimum_python[1]
        ));
    }

    let expected_names = requirements
        .iter()
        .map(|requirement| requirement.name.as_str())
        .collect::<BTreeSet<_>>();
    let actual_names = report
        .distribution_versions
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if expected_names != actual_names {
        errors.push(format!(
            "Python 协议返回的发行版集合不一致：期望 {expected_names:?}，实际 {actual_names:?}"
        ));
    }
    for requirement in requirements {
        match report.distribution_versions.get(&requirement.name) {
            None => errors.push(format!(
                "Python 协议未返回固定依赖：{}=={}",
                requirement.name, requirement.version
            )),
            Some(None) => errors.push(format!(
                "缺少固定 Python 依赖：{}=={}",
                requirement.name, requirement.version
            )),
            Some(Some(actual)) if actual != &requirement.version => errors.push(format!(
                "Python 依赖版本不匹配：{} 必须为 {}，实际为 {actual}",
                requirement.name, requirement.version
            )),
            Some(Some(_)) => {}
        }
    }
    failure(errors)
}

pub(crate) fn validate_native_probe(
    exit_code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<String> {
    if exit_code != Some(0) {
        let detail = command_detail(stdout, stderr);
        return Err(PreservedFailure::new(
            format!(
                "tree-sitter Rust 原生探针失败（exit={}）{}",
                exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "进程被信号终止".into()),
                if detail.is_empty() {
                    String::new()
                } else {
                    format!("：{detail}")
                }
            ),
            exit_code.or(Some(1)),
        )
        .into());
    }
    let text = std::str::from_utf8(stdout)
        .map_err(|error| format!("tree-sitter Rust 原生探针输出不是 UTF-8：{error}"))?;
    let marker = text.trim();
    if marker.is_empty() {
        return Err("tree-sitter Rust 原生探针成功退出但没有输出".into());
    }
    if marker != NATIVE_PROBE_MARKER {
        return Err(format!("tree-sitter Rust 原生探针返回未预期输出：{}", clip(marker)).into());
    }
    Ok(marker.to_owned())
}

pub(crate) fn command_detail(stdout: &[u8], stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr).trim().to_owned();
    let stdout = String::from_utf8_lossy(stdout).trim().to_owned();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    clip(&detail)
}

fn failure(errors: Vec<String>) -> Result<()> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(PreservedFailure::new(errors.join("\n"), Some(1)).into())
    }
}

fn clip(value: &str) -> String {
    const LIMIT: usize = 2_000;
    if value.len() <= LIMIT {
        return value.to_owned();
    }
    let mut end = LIMIT;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}
