use std::{env, ffi::OsString, path::Path, process::Command};

use super::ResourceError;

// 检查工具与生成器嵌入同一个只读判定入口，避免各自维护稳定版截止规则。
const POLICY: &str = include_str!("../../../../../scripts/release_stage.py");

pub(super) fn stable(root: &Path) -> Result<bool, ResourceError> {
    let output = Command::new(python_executable(env::var_os("RYFRAME_PYTHON")))
        .args(["-X", "utf8", "-c", POLICY, "--root"])
        .arg(root)
        .output()
        .map_err(|error| {
            ResourceError::new(
                format!("无法检查 Workspace 版本阶段：{error}"),
                "使用 scripts/check_python_environment.py 验证 Python 检查环境",
            )
        })?;
    if !output.status.success() {
        return Err(ResourceError::new(
            String::from_utf8_lossy(&output.stderr),
            "修正 Workspace 版本；稳定版生效后不得降级绕过保护",
        ));
    }
    serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .ok()
        .and_then(|value| value.get("stable").and_then(serde_json::Value::as_bool))
        .ok_or_else(|| ResourceError::new("版本阶段判定结果无效", "检查 Python 运行环境"))
}

fn python_executable(configured: Option<OsString>) -> OsString {
    configured
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("python"))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::python_executable;

    #[test]
    fn configured_python_takes_precedence() {
        let configured = OsString::from(r"D:\tools\python.exe");
        assert_eq!(
            python_executable(Some(configured.clone())),
            configured,
            "生成器必须原样保留显式解释器路径"
        );
    }

    #[test]
    fn absent_or_empty_python_uses_path_default() {
        assert_eq!(python_executable(None), OsString::from("python"));
        assert_eq!(
            python_executable(Some(OsString::new())),
            OsString::from("python")
        );
    }
}
