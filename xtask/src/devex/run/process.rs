use std::{
    collections::BTreeMap,
    path::Path,
    process::{Command, ExitStatus, Stdio},
};

use crate::Result;

use super::super::{
    metadata::corepack_executable,
    model::{StepDefinition, SuiteDefinition, WorkingDirectory},
    support::{display_step, success_status},
};

pub(super) fn execute_steps(
    backend_root: &Path,
    frontend_root: &Path,
    target: &Path,
    definition: SuiteDefinition,
    environment: &BTreeMap<String, String>,
    label: &str,
) -> Result<ExitStatus> {
    let mut last_status = success_status()?;
    for (index, step) in definition.steps.iter().enumerate() {
        println!("  → {label} step {:02}: {}", index + 1, display_step(step));
        last_status = step_command(
            step,
            backend_root,
            frontend_root,
            target,
            definition,
            environment,
            label,
        )
        .status()?;
        if !last_status.success() {
            break;
        }
    }
    Ok(last_status)
}

fn step_command(
    step: &StepDefinition,
    backend_root: &Path,
    frontend_root: &Path,
    target: &Path,
    definition: SuiteDefinition,
    environment: &BTreeMap<String, String>,
    label: &str,
) -> Command {
    let program = if step.program == "corepack" {
        corepack_executable()
    } else {
        step.program
    };
    let mut command = Command::new(program);
    let target = target.to_string_lossy();
    let frontend = frontend_root.to_string_lossy();
    let args = step
        .args
        .iter()
        .map(|arg| {
            arg.replace("{target}", &target)
                .replace("{frontend}", &frontend)
                .replace("{label}", label)
        })
        .collect::<Vec<_>>();
    command
        .args(args)
        .current_dir(match step.working_directory {
            WorkingDirectory::Backend => backend_root,
            WorkingDirectory::Frontend => frontend_root,
        })
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for key in definition.remove_environment {
        command.env_remove(key);
    }
    for (key, value) in environment {
        command.env(
            key,
            value
                .replace("{target}", &target)
                .replace("{frontend}", &frontend)
                .replace("{label}", label),
        );
    }
    if step.program == "cargo" {
        command.env("CARGO_TARGET_DIR", target.as_ref());
    }
    command
}
