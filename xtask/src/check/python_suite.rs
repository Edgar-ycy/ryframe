use std::path::Path;

use crate::{Result, process::run as run_process};

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
        let binding = crate::rate_limit::tool_binding(&executable)?;
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
