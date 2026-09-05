use super::super::{
    memory::{self, MemoryEvidence},
    metadata::corepack_executable,
    model::{StepDefinition, SuiteDefinition, WorkingDirectory},
    support::{display_step, success_status},
};
use crate::Result;
use std::{
    collections::BTreeMap,
    path::Path,
    process::{Command, ExitStatus, Stdio},
};

pub(super) fn execute_steps(
    backend_root: &Path,
    frontend_root: &Path,
    target: &Path,
    definition: SuiteDefinition,
    environment: &BTreeMap<String, String>,
    label: &str,
) -> Result<(ExitStatus, MemoryEvidence)> {
    let mut last_status = success_status()?;
    let mut memory = MemoryEvidence::new();
    for (index, step) in definition.steps.iter().enumerate() {
        println!("  → {label} step {:02}: {}", index + 1, display_step(step));
        let command = step_command(
            step,
            backend_root,
            frontend_root,
            target,
            definition,
            environment,
            label,
        );
        let (status, reading) = memory::execute(command)?;
        last_status = status;
        memory.steps.push(reading);
        if !last_status.success() {
            break;
        }
    }
    Ok((last_status, memory))
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
    let backend = backend_root.to_string_lossy();
    let driver_root = crate::workspace::root_dir();
    let driver = driver_root.to_string_lossy();
    let args = step
        .args
        .iter()
        .map(|arg| {
            arg.replace("{driver}", &driver)
                .replace("{backend}", &backend)
                .replace("{target}", &target)
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
