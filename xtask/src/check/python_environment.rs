use std::{path::Path, process::Output, time::Duration};

#[path = "python_environment/protocol.rs"]
mod protocol;

#[cfg(test)]
#[path = "../../tests/internal/python_environment.rs"]
mod tests;

use crate::{
    Result,
    process::{PreservedFailure, failure_exit_code},
};

use super::consumer_context::python;

const MINIMUM_PYTHON: [u64; 3] = [3, 11, 0];
const ENVIRONMENT_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) fn run(root: &Path) -> Result<()> {
    let program = python::selected_program(root)?;
    let requested = program.display().to_string();
    println!("→ 核验 Python 解释器：{requested}");
    let output = python::output(
        root,
        python::command(root, &program, &["-I", "-c", protocol::ENVIRONMENT_PROBE]),
        None,
        ENVIRONMENT_TIMEOUT,
    )?;
    let report = environment_report(&output, &requested)?;
    let identity = interpreter_identity(&report);
    if let Err(error) = protocol::validate_environment(MINIMUM_PYTHON, &report) {
        return Err(environment_failure(&requested, &identity, error.as_ref()));
    }
    println!("✓ Python 解释器核验通过：{identity}");

    println!(
        "Python environment check passed (executable={}, implementation={}, version={}).",
        report.executable,
        report.implementation,
        version_label(&report.version)
    );
    Ok(())
}

fn environment_report(output: &Output, requested: &str) -> Result<protocol::EnvironmentReport> {
    if !output.status.success() {
        let detail = protocol::command_detail(&output.stdout, &output.stderr);
        return Err(PreservedFailure::new(
            format!(
                "Python 解释器检查失败（exit={}）：{detail}\n\
                 配置解释器={requested}\n\
                 修复：确认 Python 3.11+ 解释器可启动，并在工具文件 verification.python_program 指定该入口。",
                output
                    .status
                    .code()
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "进程被信号终止".into())
            ),
            output.status.code().or(Some(1)),
        )
        .into());
    }
    protocol::decode_report(&output.stdout, &output.stderr).map_err(|error| {
        PreservedFailure::new(
            format!(
                "Python 环境协议失败：{error}\n配置解释器={requested}\n\
                 修复：确认解释器未被启动脚本改写，并重新运行环境检查。"
            ),
            Some(1),
        )
        .into()
    })
}

fn environment_failure(
    requested: &str,
    identity: &str,
    error: &(dyn std::error::Error + 'static),
) -> Box<dyn std::error::Error> {
    PreservedFailure::new(
        format!(
            "Python 环境核验失败：\n{error}\n\
             配置解释器={requested}\n实际解释器={identity}\n\
             修复：在工具文件 verification.python_program 指定 Python 3.11+ 并重新运行 cargo xtask check。"
        ),
        failure_exit_code(error).or(Some(1)),
    )
    .into()
}

fn interpreter_identity(report: &protocol::EnvironmentReport) -> String {
    format!(
        "{} {} ({})",
        report.implementation,
        version_label(&report.version),
        report.executable
    )
}

fn version_label(version: &[u64]) -> String {
    version
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(".")
}
