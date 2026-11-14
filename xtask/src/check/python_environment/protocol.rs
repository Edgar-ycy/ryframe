use serde::Deserialize;

use crate::{Result, process::PreservedFailure};

pub(crate) const ENVIRONMENT_PROBE: &str = r#"
import json
import os
import platform
import sys

try:
    response = {
        "executable": os.path.realpath(sys.executable),
        "implementation": platform.python_implementation(),
        "version": [sys.version_info.major, sys.version_info.minor, sys.version_info.micro],
    }
    sys.stdout.buffer.write(
        json.dumps(response, ensure_ascii=False, sort_keys=True).encode("utf-8")
    )
except Exception as error:
    sys.stderr.buffer.write(
        f"Python environment protocol failed: {type(error).__name__}\n".encode("utf-8")
    )
    raise SystemExit(2)
"#;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvironmentReport {
    pub(crate) executable: String,
    pub(crate) implementation: String,
    pub(crate) version: Vec<u64>,
}

pub(crate) fn decode_report(stdout: &[u8], stderr: &[u8]) -> Result<EnvironmentReport> {
    std::str::from_utf8(stderr)
        .map_err(|error| format!("Python 环境协议标准错误不是 UTF-8：{error}"))?;
    let text = std::str::from_utf8(stdout)
        .map_err(|error| format!("Python 环境协议输出不是 UTF-8：{error}"))?;
    if text.trim().is_empty() {
        return Err("Python 环境协议成功退出但没有输出".into());
    }
    serde_json::from_str(text)
        .map_err(|error| format!("Python 环境协议输出不是有效 JSON：{error}").into())
}

pub(crate) fn validate_environment(
    minimum_python: [u64; 3],
    report: &EnvironmentReport,
) -> Result<()> {
    let mut errors = Vec::new();
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

    failure(errors)
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
