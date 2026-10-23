use std::{fs, path::Path, process::Output, time::Duration};

#[path = "python_environment/protocol.rs"]
mod protocol;
#[path = "python_environment/requirements.rs"]
mod requirements;

#[allow(unused_imports)]
pub(crate) use protocol::{EnvironmentReport, decode_report, validate_environment};
#[allow(unused_imports)]
pub(crate) use requirements::{Requirement, parse as parse_requirements};

use crate::{
    Result,
    process::{PreservedFailure, failure_exit_code},
};

use super::consumer_context::python;

pub(crate) const REQUIREMENTS_PATH: &str = "scripts/requirements-ci.txt";

pub(crate) const MINIMUM_PYTHON: [u64; 3] = [3, 11, 0];
const ENVIRONMENT_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) fn run(root: &Path) -> Result<()> {
    let requirements_path = root.join(REQUIREMENTS_PATH);
    let requirements = read_requirements(&requirements_path)?;
    let program = python::selected_program(root)?;
    let requested = program.to_string_lossy().into_owned();
    println!(
        "→ 核验 Python 解释器与 {} 项固定依赖：{requested}",
        requirements.len()
    );
    let request = protocol::encode_request(&requirements, MINIMUM_PYTHON)?;
    let output = run_python(
        root,
        &program,
        &["-I", "-c", protocol::ENVIRONMENT_PROBE],
        Some(&request),
        ENVIRONMENT_TIMEOUT,
    )?;
    let report = environment_report(&output, &requested)?;
    let identity = interpreter_identity(&report);
    if let Err(error) = protocol::validate_environment(&requirements, MINIMUM_PYTHON, &report) {
        return Err(environment_failure(
            &requirements_path,
            &requested,
            &identity,
            error.as_ref(),
        ));
    }
    println!("✓ Python 环境与固定依赖一致：{identity}");

    println!(
        "Python environment check passed (executable={}, implementation={}, version={}, \
         distributions={}).",
        report.executable,
        report.implementation,
        version_label(&report.version),
        requirements.len()
    );
    Ok(())
}

fn read_requirements(path: &Path) -> Result<Vec<requirements::Requirement>> {
    let source = fs::read_to_string(path).map_err(|error| {
        format!(
            "无法读取固定 Python 依赖 {}：{error}；请确认从完整源码检出的后端工作树运行",
            path.display()
        )
    })?;
    requirements::parse(&path.display().to_string(), &source)
}

fn run_python(
    root: &Path,
    program: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    timeout: Duration,
) -> Result<Output> {
    python::output(root, python::command(root, program, args), input, timeout)
}

fn environment_report(output: &Output, requested: &str) -> Result<protocol::EnvironmentReport> {
    if !output.status.success() {
        let detail = protocol::command_detail(&output.stdout, &output.stderr);
        return Err(PreservedFailure::new(
            format!(
                "Python 解释器与固定依赖检查失败（exit={}）：{detail}\n\
                 配置解释器={requested}\n\
                 修复：使用 Python 3.11+ 创建隔离环境，并运行 \
                 `python -m pip install --disable-pip-version-check --require-hashes \
                 --requirement scripts/requirements-ci.txt`；在工具文件 verification.python_program 指定该环境。",
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
    protocol::decode_report(&output.stdout).map_err(|error| {
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
    requirements: &Path,
    requested: &str,
    identity: &str,
    error: &(dyn std::error::Error + 'static),
) -> Box<dyn std::error::Error> {
    PreservedFailure::new(
        format!(
            "Python 环境核验失败：\n{error}\n\
             配置解释器={requested}\n实际解释器={identity}\n\
             依赖清单={}\n\
             修复：使用 Python 3.11+ 创建隔离环境，并按依赖清单安装；\
             随后在工具文件 verification.python_program 指定该环境并重新运行 cargo xtask check。",
            requirements.display()
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
