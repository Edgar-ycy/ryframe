use std::path::Path;

use crate::{
    Result,
    cli::{FullStackCommand, RecoveryCommand},
    process::{run_owned, run_owned_with_env},
    workspace::root_dir,
};

pub(crate) fn run(command: &RecoveryCommand, frontend_dir: &Path) -> Result<()> {
    if let RecoveryCommand::FullStack(command) = command {
        return run_full_stack(command);
    }
    let (program, forwarded) = recovery_command(command, frontend_dir)?;
    let mut command = Vec::with_capacity(forwarded.len() + 1);
    command.push(program.to_owned());
    command.extend(forwarded);
    let executable = if program.ends_with(".mjs") {
        "node"
    } else {
        command.insert(0, "-B".to_owned());
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
        RecoveryCommand::Inputs(arguments) => Ok((
            "scripts/restore_input_plan.py",
            with_paths(arguments, &backend, None)?,
        )),
        RecoveryCommand::Runtime(arguments) => {
            let frontend = matches!(arguments.first().map(String::as_str), Some("build"))
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
            } else if arguments.first().map(String::as_str) == Some("review") {
                Ok((
                    "scripts/reference_fixture_review.py",
                    with_paths(&arguments[1..], &backend, None)?,
                ))
            } else if arguments.first().map(String::as_str) == Some("request") {
                Ok((
                    "scripts/reference_fixture_request.py",
                    with_paths(&arguments[1..], &backend, None)?,
                ))
            } else if arguments.first().map(String::as_str) == Some("successor") {
                Ok((
                    "scripts/reference_fixture_successor.py",
                    with_paths(&arguments[1..], &backend, None)?,
                ))
            } else if arguments.first().map(String::as_str) == Some("artifact") {
                Ok((
                    "scripts/full_stack_artifacts.py",
                    with_paths(&arguments[1..], &backend, None)?,
                ))
            } else if arguments.first().map(String::as_str) == Some("retention") {
                Ok((
                    "scripts/full_stack_migration_history.py",
                    with_paths(&arguments[1..], &backend, None)?,
                ))
            } else if arguments.first().map(String::as_str) == Some("services") {
                Ok((
                    "scripts/reference_fixture_services.py",
                    with_paths(&arguments[1..], &backend, None)?,
                ))
            } else if arguments.first().map(String::as_str) == Some("source-pair") {
                Ok((
                    "scripts/reference_fixture_source_pair.py",
                    with_paths(&arguments[1..], &backend, None)?,
                ))
            } else if arguments.first().map(String::as_str) == Some("runtime") {
                Ok((
                    "scripts/reference_fixture_runtime.py",
                    with_paths(&arguments[1..], &backend, None)?,
                ))
            } else if arguments.first().map(String::as_str) == Some("dataset") {
                Ok((
                    "scripts/reference_fixture_dataset.py",
                    with_paths(&arguments[1..], &backend, None)?,
                ))
            } else {
                Ok((
                    "scripts/prepare_full_stack_fixture.py",
                    with_paths(arguments, &backend, Some(frontend.as_str()))?,
                ))
            }
        }
        RecoveryCommand::FullStack(_) => Ok(("scripts/ci_full_stack.py", Vec::new())),
        RecoveryCommand::Monitoring(arguments) => Ok((
            "scripts/restore_monitoring_delivery.py",
            with_paths(arguments, &backend, None)?,
        )),
        RecoveryCommand::DatasetPrepare(arguments) => Ok((
            "scripts/restore_reference_dataset.mjs",
            with_paths(arguments, &backend, None)?,
        )),
    }
}

fn run_full_stack(command: &FullStackCommand) -> Result<()> {
    match command {
        FullStackCommand::Help => {
            println!("cargo xtask check recovery full-stack <prepare|rate-limit|start|collect>");
            return Ok(());
        }
        FullStackCommand::RateLimitHelp => {
            println!(
                "cargo xtask check recovery full-stack rate-limit --environment-file <绝对文件>"
            );
            return Ok(());
        }
        _ => {}
    }
    let backend = path_argument(&root_dir(), "后端目录")?;
    let environment = full_stack_environment(command, &backend)?;
    run_owned_with_env(
        &root_dir(),
        "python",
        &strings(&["-B", "scripts/ci_full_stack.py"]),
        &environment,
    )
}

pub(crate) fn full_stack_environment(
    command: &FullStackCommand,
    backend: &str,
) -> Result<Vec<(&'static str, String)>> {
    let operation = match command {
        FullStackCommand::Prepare => "prepare",
        FullStackCommand::Start => "start",
        FullStackCommand::Collect => "collect",
        FullStackCommand::RateLimit { .. } => "rate-limit",
        FullStackCommand::Help | FullStackCommand::RateLimitHelp => {
            return Err("full-stack 帮助不启动私有阶段程序".into());
        }
    };
    let mut environment = vec![
        ("RYFRAME_XTASK_FULL_STACK_OPERATION", operation.to_owned()),
        ("RYFRAME_XTASK_BACKEND_ROOT", backend.to_owned()),
    ];
    if let FullStackCommand::RateLimit { environment_file } = command {
        environment.push((
            "RYFRAME_XTASK_FULL_STACK_ENVIRONMENT_FILE",
            path_argument(environment_file, "环境文件")?,
        ));
    }
    Ok(environment)
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
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
