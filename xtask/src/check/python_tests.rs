use std::path::Path;

use serde_json::json;

use crate::{
    Result,
    process::{run_with_env, run_with_env_removed},
    recovery::clone::native::digest,
};

use super::PYTHON_TEST_ARGS;

const TOOL_ENVIRONMENT: &str = "RYFRAME_RATE_LIMIT_TOOL_BINDING";

/// 兼容测试复用当前已构建的工具，配置内容仍通过每次独立调用重新加载。
pub(super) fn run(root: &Path) -> Result<()> {
    let executable = std::env::current_exe()?;
    let Some(binding) = binding(&executable)? else {
        return run_with_env_removed(root, "python", PYTHON_TEST_ARGS, &[], &[TOOL_ENVIRONMENT]);
    };
    run_with_env(
        root,
        "python",
        PYTHON_TEST_ARGS,
        &[(TOOL_ENVIRONMENT, &binding)],
    )
}

pub(crate) fn binding(executable: &Path) -> Result<Option<String>> {
    let path = executable.to_str().ok_or("当前检查工具路径不是 UTF-8")?;
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    // 既有 Windows 二进制保护仅支持本机盘符；其他布局继续使用原 Cargo 入口。
    if cfg!(windows) && (path.starts_with("UNC\\") || path.starts_with(r"\\")) {
        return Ok(None);
    }
    if digest::linked_path(executable)?
        .into_iter()
        .any(|linked| linked)
    {
        return Ok(None);
    }
    let observed = digest::file(executable)?;
    Ok(Some(serde_json::to_string(&json!({
        "format_version": 1,
        "kind": "ryframe-native-rate-limit-tool",
        "path": path,
        "bytes": observed.bytes,
        "sha256": observed.sha256,
    }))?))
}
