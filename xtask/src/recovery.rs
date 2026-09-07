use std::path::Path;

use crate::{Result, cli::RecoveryCommand, process::run_owned, workspace::root_dir};

pub(crate) fn run(command: &RecoveryCommand, frontend_dir: &Path) -> Result<()> {
    let (program, forwarded) = recovery_command(command, frontend_dir)?;
    let mut command = Vec::with_capacity(forwarded.len() + 1);
    command.push(program.to_owned());
    command.extend(forwarded);
    let executable = if program.ends_with(".mjs") {
        "node"
    } else {
        "python"
    };
    run_owned(&root_dir(), executable, &command)
}

pub(crate) fn recovery_command(
    command: &RecoveryCommand,
    frontend_dir: &Path,
) -> Result<(&'static str, Vec<String>)> {
    let backend = path_argument(&root_dir(), "后端目录")?;
    let frontend = path_argument(frontend_dir, "前端目录")?;
    match command {
        RecoveryCommand::Reference(arguments) => Ok((
            "scripts/restore_reference.py",
            with_paths(arguments, &backend, None)?,
        )),
        RecoveryCommand::Runtime(arguments) => {
            let frontend = matches!(
                arguments.first().map(String::as_str),
                Some("bind" | "verify")
            )
            .then_some(frontend.as_str());
            Ok((
                "scripts/restore_runtime.py",
                with_paths(arguments, &backend, frontend)?,
            ))
        }
        RecoveryCommand::Source(arguments) => Ok((
            "scripts/restore_source.py",
            with_paths(arguments, &backend, None)?,
        )),
        RecoveryCommand::Clone(arguments) => Ok((
            "scripts/devex_clone.py",
            with_paths(arguments, &backend, None)?,
        )),
        RecoveryCommand::FreshTarget(arguments) => {
            let mut forwarded = arguments.clone();
            forwarded.insert(0, "fresh-target".to_owned());
            Ok((
                "scripts/devex_clone.py",
                with_paths(&forwarded, &backend, None)?,
            ))
        }
        RecoveryCommand::Fixture(arguments) => {
            if arguments.first().map(String::as_str) == Some("environment") {
                Ok((
                    "scripts/reference_fixture_environment.py",
                    with_paths(&arguments[1..], &backend, None)?,
                ))
            } else {
                Ok((
                    "scripts/prepare_full_stack_fixture.py",
                    with_paths(arguments, &backend, Some(frontend.as_str()))?,
                ))
            }
        }
        RecoveryCommand::DatasetPrepare(arguments) => Ok((
            "scripts/restore_reference_dataset.mjs",
            with_paths(arguments, &backend, None)?,
        )),
    }
}

fn path_argument(path: &Path, label: &str) -> Result<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{label}必须能表示为 UTF-8 命令参数").into())
}

fn with_paths(arguments: &[String], backend: &str, frontend: Option<&str>) -> Result<Vec<String>> {
    if arguments.iter().any(|value| value == "--backend-dir") {
        return Err("恢复验收由 cargo xtask 固定当前后端目录，不接受 --backend-dir".into());
    }
    if frontend.is_some() && arguments.iter().any(|value| value == "--frontend-dir") {
        return Err("恢复验收由 cargo xtask 固定当前前端目录，不接受 --frontend-dir".into());
    }
    let mut forwarded = Vec::with_capacity(arguments.len() + 4);
    forwarded.extend(arguments.iter().cloned());
    forwarded.push("--backend-dir".to_owned());
    forwarded.push(backend.to_owned());
    if let Some(frontend) = frontend {
        forwarded.push("--frontend-dir".to_owned());
        forwarded.push(frontend.to_owned());
    }
    Ok(forwarded)
}
