use std::path::Path;

use crate::{Result, process::run as run_process};

use super::{PYTHON_TEST_ARGS, consumer_context::python};

/// Python 检查显式绑定当前原生 rate-limit 工具。
pub(super) fn run(root: &Path, report_test_names: bool) -> Result<()> {
    let program = python::selected_program(root)?;
    let program = program.to_str().ok_or("Python 入口路径必须为 UTF-8。")?;
    let executable = std::env::current_exe()?;
    let binding = crate::rate_limit::tool_binding(&executable)?;
    let arguments = arguments(binding.as_deref(), report_test_names);
    let arguments = arguments.iter().map(String::as_str).collect::<Vec<_>>();
    run_process(root, program, &arguments)
}

/// 工具描述使用单独参数传递，不进入应用配置或子进程环境。
pub(crate) fn arguments(binding: Option<&str>, report_test_names: bool) -> Vec<String> {
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
    if report_test_names {
        // CI 先报告当前用例名称，保留未结束用例的定位线索。
        arguments.push("-v".to_owned());
    }
    arguments
}
