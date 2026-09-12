use std::path::Path;

use crate::{
    Result,
    cli::{
        FreshTargetCommand, FreshTargetOptions, FullStackCommand, RecoveryCommand,
        SeedSourceOptions,
    },
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    process::{run_owned, run_owned_with_env, run_with_env_removed},
    workspace::root_dir,
};

const FRESH_TARGET_PROTOCOL_ENV: &str = "RYFRAME_XTASK_RECOVERY_FRESH_TARGET";
const SEED_SOURCE_PROTOCOL_ENV: &str = "RYFRAME_XTASK_RECOVERY_SEED_SOURCE";

#[path = "recovery/dataset_prepare.rs"]
pub(crate) mod dataset_prepare;
#[path = "recovery/fixture_control.rs"]
pub(crate) mod fixture_control;
#[path = "recovery/fixture_runtime.rs"]
pub(crate) mod fixture_runtime;
#[path = "recovery/monitoring.rs"]
pub(crate) mod monitoring;

pub(crate) fn run(command: &RecoveryCommand, frontend_dir: &Path) -> Result<()> {
    if let RecoveryCommand::FullStack(command) = command {
        return run_full_stack(command);
    }
    if let RecoveryCommand::FreshTarget(command) = command {
        return run_fresh_target(command);
    }
    if let RecoveryCommand::FixtureRuntime(command) = command {
        return fixture_runtime::run(command, &root_dir());
    }
    if let RecoveryCommand::SeedSource(options) = command {
        return run_seed_source(options);
    }
    if let RecoveryCommand::DatasetPrepare(command) = command {
        return dataset_prepare::run(command, &root_dir());
    }
    if let RecoveryCommand::Monitoring(command) = command {
        return monitoring::run(command, &root_dir());
    }
    if let RecoveryCommand::FixtureControl(command) = command {
        return fixture_control::run(command, &root_dir());
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
        RecoveryCommand::SeedSource(_) => {
            Err("seed source 必须通过版本化私有协议执行，不能透传 argv"
                .to_owned()
                .into())
        }
        RecoveryCommand::FreshTarget(_) => {
            Err("fresh-target 必须通过版本化私有协议执行，不能透传 argv"
                .to_owned()
                .into())
        }
        RecoveryCommand::Fixture(arguments) => {
            if arguments.first().is_some_and(|value| {
                ["environment", "review", "request", "successor", "services"]
                    .contains(&value.as_str())
            }) {
                Err("fixture 控制子域不能通过未解析参数绕过版本化私有协议"
                    .to_owned()
                    .into())
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
            } else if arguments.first().map(String::as_str) == Some("source-pair") {
                Ok((
                    "scripts/reference_fixture_source_pair.py",
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
        RecoveryCommand::FixtureControl(_) => {
            Err("fixture 控制请求必须通过版本化私有协议执行，不能透传 argv"
                .to_owned()
                .into())
        }
        RecoveryCommand::FixtureRuntime(_) => {
            Ok(("scripts/reference_fixture_runtime.py", Vec::new()))
        }
        RecoveryCommand::FullStack(_) => Ok(("scripts/ci_full_stack.py", Vec::new())),
        RecoveryCommand::Monitoring(_) => {
            Err("monitoring 必须通过版本化私有协议执行，不能透传 argv"
                .to_owned()
                .into())
        }
        RecoveryCommand::DatasetPrepare(_) => {
            Err("dataset-prepare 必须通过版本化私有协议执行，不能透传 argv"
                .to_owned()
                .into())
        }
    }
}

fn run_seed_source(options: &SeedSourceOptions) -> Result<()> {
    let root = root_dir();
    let payload = seed_source_protocol_at(options, &root)?;
    run_with_env_removed(
        &root,
        "python",
        &["-B", "scripts/devex_clone.py"],
        &[(SEED_SOURCE_PROTOCOL_ENV, payload.as_str())],
        &[FRESH_TARGET_PROTOCOL_ENV, SEED_SOURCE_PROTOCOL_ENV],
    )
}

pub(crate) fn seed_source_protocol_at(options: &SeedSourceOptions, root: &Path) -> Result<String> {
    validate_local_test_path(&options.run_dir, root, LocalTestPathKind::ExistingDirectory)?;
    if let Some(request) = &options.request {
        validate_local_test_path(request, root, LocalTestPathKind::ExistingFile)?;
    }
    let protocol = serde_json::json!({
        "format_version": 1,
        "kind": "ryframe-xtask-recovery-seed-source",
        "request": {
            "backend_dir": path_argument(root, "后端目录")?,
            "operation": options.operation.as_str(),
            "run_dir": path_argument(&options.run_dir, "seed source 运行目录")?,
            "request": options.request.as_deref()
                .map(|value| path_argument(value, "seed source 请求"))
                .transpose()?,
            "write": options.write,
        }
    });
    let payload = serde_json::to_string(&protocol)?;
    if payload.contains(['\r', '\n', '\0']) {
        return Err("seed source 私有协议不能包含换行符或 NUL".into());
    }
    Ok(payload)
}

fn run_fresh_target(command: &FreshTargetCommand) -> Result<()> {
    let FreshTargetCommand::Run(options) = command else {
        println!(
            "cargo xtask check recovery fresh-target --workspace <.local-tests 子目录> \
             --operation <prepare|resume-prepare|initialize|resume-initialize|reconcile-preflight|verify|status> \
             [阶段参数] [--write]"
        );
        return Ok(());
    };
    let payload = fresh_target_protocol(options)?;
    run_with_env_removed(
        &root_dir(),
        "python",
        &["-B", "scripts/devex_clone.py"],
        &[(FRESH_TARGET_PROTOCOL_ENV, payload.as_str())],
        &[FRESH_TARGET_PROTOCOL_ENV, SEED_SOURCE_PROTOCOL_ENV],
    )
}

pub(crate) fn fresh_target_protocol(options: &FreshTargetOptions) -> Result<String> {
    let path = |value: &Path| path_argument(value, "fresh-target 路径");
    let optional_path = |value: &Option<std::path::PathBuf>| -> Result<Option<String>> {
        value.as_deref().map(path).transpose()
    };
    Ok(serde_json::to_string(&serde_json::json!({
        "format_version": 1,
        "kind": "ryframe-xtask-recovery-fresh-target",
        "request": {
            "backend_dir": path(&root_dir())?,
            "operation": options.operation.as_str(),
            "workspace": path(&options.workspace)?,
            "request": optional_path(&options.request)?,
            "environment": optional_path(&options.environment)?,
            "storage_run": optional_path(&options.storage_run)?,
            "observation_dir": optional_path(&options.observation_dir)?,
            "write": options.write,
        }
    }))?)
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
