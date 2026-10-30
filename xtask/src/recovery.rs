#[path = "recovery/frontend_build.rs"]
pub(crate) mod frontend_build;
use std::path::Path;

use crate::{
    Result,
    cli::{
        CloneCommand, FreshTargetCommand, RecoveryCommand, SeedSourceOperation, SeedSourceOptions,
    },
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    process::{protocol_stdin, run_with_stdin},
    workspace::root_dir,
};

#[path = "recovery/clone.rs"]
pub(crate) mod clone;

#[path = "recovery/dataset_prepare.rs"]
pub(crate) mod dataset_prepare;
#[path = "recovery/fixture_control.rs"]
pub(crate) mod fixture_control;
#[path = "recovery/fixture_prepare.rs"]
pub(crate) mod fixture_prepare;
#[path = "recovery/fixture_runtime.rs"]
pub(crate) mod fixture_runtime;
#[path = "recovery/full_stack.rs"]
pub(crate) mod full_stack;
#[path = "recovery/inputs/mod.rs"]
pub(crate) mod inputs;
#[path = "recovery/monitoring.rs"]
pub(crate) mod monitoring;
#[path = "recovery/reference.rs"]
pub(crate) mod reference;
#[path = "recovery/runtime.rs"]
pub(crate) mod runtime;
#[path = "recovery/source.rs"]
pub(crate) mod source;

pub(crate) fn run(command: &RecoveryCommand, frontend_dir: &Path) -> Result<()> {
    match command {
        RecoveryCommand::Reference(command) => reference::run(command, &root_dir()),
        RecoveryCommand::Inputs(command) => inputs::run(command, &root_dir()),
        RecoveryCommand::Runtime(command) => runtime::run(command, &root_dir(), frontend_dir),
        RecoveryCommand::Source(command) => source::run(command, &root_dir()),
        RecoveryCommand::Clone(command) => run_clone(command),
        RecoveryCommand::SeedSource(options) => run_seed_source(options),
        RecoveryCommand::FreshTarget(command) => run_fresh_target(command),
        RecoveryCommand::FixturePrepare(command) => {
            fixture_prepare::run(command, &root_dir(), frontend_dir)
        }
        RecoveryCommand::FixtureControl(command) => fixture_control::run(command, &root_dir()),
        RecoveryCommand::FixtureRuntime(command) => fixture_runtime::run(command, &root_dir()),
        RecoveryCommand::FullStack(command) => full_stack::run(command, &root_dir()),
        RecoveryCommand::Monitoring(command) => monitoring::run(command, &root_dir()),
        RecoveryCommand::DatasetPrepare(command) => dataset_prepare::run(command, &root_dir()),
    }
}

fn clone_stdin(payload: &str) -> Result<std::process::Stdio> {
    if payload.len() > 64 * 1024 {
        return Err("复制私有协议超过 64 KiB".into());
    }
    protocol_stdin(Some(payload.as_bytes()))
}

fn run_seed_source(options: &SeedSourceOptions) -> Result<()> {
    let root = root_dir();
    if matches!(
        options.operation,
        SeedSourceOperation::GenerationStop
            | SeedSourceOperation::GenerationRecover
            | SeedSourceOperation::GenerationStatus
            | SeedSourceOperation::GenerationStart
    ) {
        let result = source::evidence::control::run(&root, options)?;
        println!("{}", serde_json::to_string(&result)?);
        return Ok(());
    }
    if options.operation == SeedSourceOperation::Rebind {
        let result = clone::native::seed::rebind::run(&root, options)?;
        println!("{}", serde_json::to_string(&result)?);
        return Ok(());
    }
    if options.operation == SeedSourceOperation::Register {
        if !options.write || options.request.is_some() {
            return Err("来源登记要求 --write，且不接受外部请求文件".into());
        }
        let runner =
            clone::native::seed::source_registration::runner::SourceRegisterRunner::new(&root);
        let result = clone::native::lifecycle::stage::run_named(
            &root,
            &options.run_dir,
            "seed-runtime",
            "source-register",
            &runner,
        )?;
        println!("{}", serde_json::to_string(&result)?);
        return Ok(());
    }
    let payload = seed_source_protocol_at(options, &root)?;
    run_with_stdin(
        &root,
        "python",
        &["-B", "tools/python/devex_clone.py", "--private-stdin"],
        clone_stdin(&payload)?,
    )
}

fn run_clone(command: &CloneCommand) -> Result<()> {
    match command {
        CloneCommand::Help => {
            println!("{}", crate::cli::CLONE_USAGE);
            return Ok(());
        }
        CloneCommand::Plan { .. }
        | CloneCommand::Verify { .. }
        | CloneCommand::Init { .. }
        | CloneCommand::Status { .. }
        | CloneCommand::Stage { .. }
        | CloneCommand::Recover { .. }
        | CloneCommand::Bridge { .. }
        | CloneCommand::MaintenanceBuild { .. }
        | CloneCommand::MaintenanceVerify { .. }
        | CloneCommand::Storage(_) => {
            return clone::native::run(command, &root_dir());
        }
        _ => {}
    }
    let root = root_dir();
    let payload = clone::protocol(command, &root)?;
    run_with_stdin(
        &root,
        "python",
        &["-B", "tools/python/devex_clone.py", "--private-stdin"],
        clone_stdin(&payload)?,
    )
}

pub(crate) fn seed_source_protocol_at(options: &SeedSourceOptions, root: &Path) -> Result<String> {
    if matches!(
        options.operation,
        SeedSourceOperation::Register | SeedSourceOperation::Rebind
    ) {
        return Err("来源登记与重绑定仅通过原生 Rust 阶段入口执行，不生成 Python 私有协议".into());
    }
    validate_local_test_path(&options.run_dir, root, LocalTestPathKind::ExistingDirectory)?;
    if let Some(request) = &options.request {
        validate_local_test_path(request, root, LocalTestPathKind::ExistingFile)?;
    }
    let protocol = serde_json::json!({
        "format_version": 1,
        "kind": "ryframe-xtask-recovery-seed-source",
        "native_tool": crate::check::python_suite::native_tool_binding()?,
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
             [阶段参数] [--write]\n  resume-initialize 仅续作已证明的库存失败，或首个缺失操作为只读 verify 的完整迁移前缀"
        );
        return Ok(());
    };
    if options.operation.as_str() == "status" {
        let value = clone::native::target_verify::fresh::observe(&root_dir(), &options.workspace)?;
        println!("{}", serde_json::to_string(&value)?);
        return Ok(());
    }
    let value = clone::native::target_verify::fresh::dispatch(&root_dir(), options)?;
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}

fn path_argument(path: &Path, label: &str) -> Result<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{label}必须能表示为 UTF-8 命令参数").into())
}
