use std::path::Path;

use serde_json::json;

use crate::{Result, process::run as run_process, recovery::clone::native::digest};

use super::{
    PYTHON_TEST_ARGS,
    consumer_context::{prepare_python_origin, python},
};

/// Python 检查持有已核验来源的 origin 绑定，并复用当前原生 rate-limit 工具。
pub(super) fn run(root: &Path) -> Result<()> {
    let origin = prepare_python_origin(root)?;
    origin.verify()?;
    let result = (|| {
        let program = python::selected_program(root)?;
        let program = program.to_str().ok_or("Python 入口路径必须为 UTF-8。")?;
        let executable = std::env::current_exe()?;
        let binding = binding(&executable)?;
        let arguments = arguments(binding.as_deref());
        let arguments = arguments.iter().map(String::as_str).collect::<Vec<_>>();
        run_process(root, program, &arguments)
    })();
    origin.finish(result)
}

/// 工具描述使用单独参数传递，不进入应用配置或子进程环境。
pub(crate) fn arguments(binding: Option<&str>) -> Vec<String> {
    let mut arguments = ["-X", "utf8", "-B", "tools/python/run_checks.py"]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if let Some(binding) = binding {
        arguments.extend(["--rate-limit-tool-binding".to_owned(), binding.to_owned()]);
    }
    arguments.push("--".to_owned());
    arguments.extend(
        PYTHON_TEST_ARGS
            .iter()
            .map(|argument| (*argument).to_owned()),
    );
    arguments
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
