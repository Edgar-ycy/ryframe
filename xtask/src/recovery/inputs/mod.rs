use std::path::Path;

use serde_json::Value;

use crate::{Result, cli::RecoveryInputsCommand};

mod arm;
mod backup;
pub(crate) mod bindings;
pub(crate) mod chain;
mod comparison;
pub(crate) mod document;
pub(in crate::recovery) mod domain;
mod fresh;
pub(in crate::recovery) mod inventory;
pub(crate) mod product;
pub(crate) mod publish;
pub(crate) mod reference;
mod reference_configuration;
mod source;
mod target;
pub(crate) mod target_plan;

pub(crate) const USAGE: &str = "cargo xtask check recovery inputs reference --arm-input <文件> --fresh-target-verify <文件> --side <base|candidate> --id <ID> --work-dir <新目录> [--output <新文件> --write]\n  cargo xtask check recovery inputs product --reference-plan <文件> --backup-receipt <文件> --comparison-sources <文件> --arm-input <文件> --fresh-target-verify <文件> --side <base|candidate> --id <ID> --fault-at <UTC Z 时间> [--output <新文件> --write]\n  cargo xtask check recovery inputs bindings --reference-plan <文件> --target-plan <文件> --backup-receipt <文件> --record <文件> [--output <新文件> --write]";

pub(crate) fn run(command: &RecoveryInputsCommand, root: &Path) -> Result<()> {
    let (operation, output, value) = match command {
        RecoveryInputsCommand::Help => {
            println!("{USAGE}");
            return Ok(());
        }
        RecoveryInputsCommand::Reference(options) => (
            "reference",
            options
                .publication
                .as_ref()
                .map(|value| value.output.as_path()),
            reference::build(root, options)?,
        ),
        RecoveryInputsCommand::Product(options) => (
            "product",
            options
                .publication
                .as_ref()
                .map(|value| value.output.as_path()),
            product::build(root, options)?,
        ),
        RecoveryInputsCommand::Bindings(options) => (
            "bindings",
            options
                .publication
                .as_ref()
                .map(|value| value.output.as_path()),
            bindings::build(root, options)?,
        ),
    };
    if let Some(target) = output {
        crate::local_test_path::validate_local_test_path(
            target,
            root,
            crate::local_test_path::LocalTestPathKind::OutputFile,
        )?;
    }
    publish::finish(operation, output, &value, || rebuild(command, root))
}

fn rebuild(command: &RecoveryInputsCommand, root: &Path) -> Result<Value> {
    match command {
        RecoveryInputsCommand::Help => Err("恢复输入帮助没有可重建结果".into()),
        RecoveryInputsCommand::Reference(options) => reference::build(root, options),
        RecoveryInputsCommand::Product(options) => product::build(root, options),
        RecoveryInputsCommand::Bindings(options) => bindings::build(root, options),
    }
}
