#[path = "recovery/b0.rs"]
pub(crate) mod b0;
#[path = "recovery/frontend_build.rs"]
pub(crate) mod frontend_build;
use std::path::Path;

use crate::{
    Result,
    cli::{
        CloneCommand, FreshTargetCommand, RecoveryCommand, SeedSourceOperation, SeedSourceOptions,
    },
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
#[path = "recovery/process_tree.rs"]
pub(in crate::recovery) mod process_tree;
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

fn run_seed_source(options: &SeedSourceOptions) -> Result<()> {
    let root = root_dir();
    let result = match options.operation {
        SeedSourceOperation::GenerationStop
        | SeedSourceOperation::GenerationRecover
        | SeedSourceOperation::GenerationStatus
        | SeedSourceOperation::GenerationStart => source::evidence::control::run(&root, options)?,
        SeedSourceOperation::GenerationRebootStatus => {
            source::evidence::reboot_status(&root, options)?
        }
        SeedSourceOperation::GenerationRebootClose => {
            source::evidence::reboot_close(&root, options)?
        }
        SeedSourceOperation::Rebind => clone::native::seed::rebind::run(&root, options)?,
        SeedSourceOperation::Export | SeedSourceOperation::ExportReconcile => {
            if !options.write || options.request.is_some() {
                return Err("seed 共享导出及采用要求 --write，且不接受外部请求".into());
            }
            let mut result = clone::native::lifecycle::stage::run_named(
                &root,
                &options.run_dir,
                "seed-runtime",
                options.operation.as_str(),
                &clone::native::seed::shared_export::Runner,
            )?;
            let _ = result
                .as_object_mut()
                .ok_or("共享导出缺少统一阶段结果")?
                .remove("result");
            result
        }
        SeedSourceOperation::Register => {
            if !options.write || options.request.is_some() {
                return Err("来源登记要求 --write，且不接受外部请求文件".into());
            }
            let runner =
                clone::native::seed::source_registration::runner::SourceRegisterRunner::new(&root);
            clone::native::lifecycle::stage::run_named(
                &root,
                &options.run_dir,
                "seed-runtime",
                "source-register",
                &runner,
            )?
        }
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

fn run_clone(command: &CloneCommand) -> Result<()> {
    let root = root_dir();
    match command {
        CloneCommand::Help => {
            println!("{}", crate::cli::CLONE_USAGE);
            Ok(())
        }
        _ => clone::native::run(command, &root),
    }
}

pub(crate) fn run_clone_exec_if_requested() -> Option<i32> {
    use clone::native::lifecycle::runtime_spawn;
    let mut arguments = std::env::args().skip(1);
    if arguments.next()?.as_str() != runtime_spawn::EXEC_ENTRY {
        return None;
    }
    Some(
        match runtime_spawn::run_exec(&arguments.collect::<Vec<_>>()) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("旧复制运行产品启动失败：{error}");
                1
            }
        },
    )
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
