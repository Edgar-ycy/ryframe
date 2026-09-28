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
    runner_frontend_root: &Path,
    target: &Path,
    definition: SuiteDefinition,
    environment: &BTreeMap<String, String>,
    label: &str,
) -> Result<(ExitStatus, MemoryEvidence)> {
    let roots = StepRoots {
        backend: backend_root,
        frontend: frontend_root,
        runner_frontend: runner_frontend_root,
        target,
    };
    let mut last_status = success_status()?;
    let mut memory = MemoryEvidence::new();
    for (index, step) in definition.steps.iter().enumerate() {
        println!("  → {label} step {:02}: {}", index + 1, display_step(step));
        let command = step_command(step, roots, definition, environment, label);
        let (status, reading) = memory::execute(command)?;
        last_status = status;
        memory.steps.push(reading);
        if !last_status.success() {
            break;
        }
    }
    Ok((last_status, memory))
}

#[derive(Clone, Copy)]
struct StepRoots<'a> {
    backend: &'a Path,
    frontend: &'a Path,
    runner_frontend: &'a Path,
    target: &'a Path,
}

fn step_command(
    step: &StepDefinition,
    roots: StepRoots<'_>,
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
    let target = roots.target.to_string_lossy();
    let frontend = roots.frontend.to_string_lossy();
    let runner_frontend = roots.runner_frontend.to_string_lossy();
    let backend = roots.backend.to_string_lossy();
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
                .replace("{runner-frontend}", &runner_frontend)
                .replace("{label}", label)
        })
        .collect::<Vec<_>>();
    command
        .args(args)
        .current_dir(match step.working_directory {
            WorkingDirectory::Backend => roots.backend,
            WorkingDirectory::Frontend => roots.frontend,
            WorkingDirectory::RunnerFrontend => roots.runner_frontend,
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
                .replace("{runner-frontend}", &runner_frontend)
                .replace("{label}", label),
        );
    }
    if step.program == "cargo" {
        command.env("CARGO_TARGET_DIR", target.as_ref());
    }
    command
}
