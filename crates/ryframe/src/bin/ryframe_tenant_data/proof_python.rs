use ryframe_kernel::{AppError, AppResult};
use std::path::PathBuf;

pub(super) const PYTHON_DEPENDENCY_PROBE: &str = r#"
import importlib.metadata as metadata
import json
import os
import platform
import sys
import yaml

yaml.safe_load("probe: true")
print(json.dumps({
    "executable": os.path.realpath(sys.executable),
    "implementation": platform.python_implementation(),
    "version": list(sys.version_info[:3]),
    "pyyaml": metadata.version("PyYAML"),
}))
"#;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PythonEnvironment {
    executable: PathBuf,
    implementation: String,
    version: [u32; 3],
    pyyaml: String,
}

pub(super) fn validate_python_environment(
    selected: &str,
    expected_pyyaml: &str,
    report: &PythonEnvironment,
) -> AppResult<()> {
    let identity = std::fs::canonicalize(selected)
        .map_err(|error| validation(format!("无法核对 Python 解释器身份：{error}")))?;
    let observed = std::fs::canonicalize(&report.executable)
        .map_err(|error| validation(format!("Python 实际解释器身份无效：{error}")))?;
    if identity != observed || report.implementation.trim().is_empty() {
        return Err(validation("Python 环境返回的解释器身份不一致"));
    }
    if report.version < [3, 11, 0] {
        return Err(validation("Python 版本过低：至少需要 3.11"));
    }
    if report.pyyaml != expected_pyyaml {
        return Err(validation(format!(
            "Python 依赖版本不匹配：PyYAML 必须为 {expected_pyyaml}，实际为 {}",
            report.pyyaml
        )));
    }
    Ok(())
}

fn validation(message: impl Into<String>) -> AppError {
    AppError::Validation(message.into())
}
