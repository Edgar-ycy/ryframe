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

#[cfg(test)]
mod tests {
    use super::*;

    fn report(executable: PathBuf) -> PythonEnvironment {
        PythonEnvironment {
            executable,
            implementation: "CPython".into(),
            version: [3, 12, 7],
            pyyaml: "6.0.3".into(),
        }
    }

    #[test]
    fn remaining_dependency_check_rejects_wrong_identity_version_and_pyyaml() {
        let executable = std::env::current_exe().unwrap();
        let selected = executable.to_str().unwrap();
        let mut observed = report(executable.clone());
        assert!(validate_python_environment(selected, "6.0.3", &observed).is_ok());
        observed.version = [3, 10, 9];
        assert!(
            validate_python_environment(selected, "6.0.3", &observed)
                .unwrap_err()
                .to_string()
                .contains("版本过低")
        );
        observed.version = [3, 12, 7];
        observed.pyyaml = "6.0.2".into();
        assert!(
            validate_python_environment(selected, "6.0.3", &observed)
                .unwrap_err()
                .to_string()
                .contains("PyYAML")
        );
        observed.pyyaml = "6.0.3".into();
        observed.executable = executable.parent().unwrap().to_owned();
        assert!(
            validate_python_environment(selected, "6.0.3", &observed)
                .unwrap_err()
                .to_string()
                .contains("身份不一致")
        );
        observed.executable = executable.clone();
        observed.implementation.clear();
        assert!(validate_python_environment(selected, "6.0.3", &observed).is_err());
    }

    #[test]
    fn dependency_protocol_rejects_empty_missing_and_extra_fields() {
        for source in [b"".as_slice(), b"{}", b"{\"unknown\":true}"] {
            assert!(serde_json::from_slice::<PythonEnvironment>(source).is_err());
        }
    }

    #[test]
    #[ignore = "需要固定 Python 3.11+ 和 PyYAML 运行环境"]
    fn remaining_dependencies_are_really_imported() {
        let output = std::process::Command::new("python")
            .args(["-I", "-B", "-c", PYTHON_DEPENDENCY_PROBE])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let observed = serde_json::from_slice::<PythonEnvironment>(&output.stdout).unwrap();
        validate_python_environment(observed.executable.to_str().unwrap(), "6.0.3", &observed)
            .unwrap();
    }
}
