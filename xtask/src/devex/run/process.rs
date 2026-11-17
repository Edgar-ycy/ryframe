use super::super::{
    memory::{self, MemoryEvidence},
    metadata::corepack_executable,
    model::{
        PAIRED_SAVE_OUTPUT, PairingMetadata, RESOURCE_GATE_DECISION_TEMPLATE, StepDefinition,
        SuiteDefinition, WorkingDirectory, is_common_frontend_fast, step_arguments,
        uses_historical_save_protocol,
    },
    support::{display_step, frontend_fast_arguments, success_status},
};
use crate::Result;
use std::{
    collections::BTreeMap,
    path::Path,
    process::{Command, ExitStatus, Stdio},
};

pub(super) struct StepInvocation<'a> {
    pub(super) variant: &'a str,
    pub(super) label: &'a str,
    pub(super) runtime_input: Option<&'a super::super::runtime_native::Input>,
    pub(super) pairing: Option<&'a PairingMetadata>,
}

pub(super) fn execute_steps(
    backend_root: &Path,
    frontend_root: &Path,
    runner_frontend_root: &Path,
    target: &Path,
    definition: SuiteDefinition,
    environment: &BTreeMap<String, String>,
    invocation: StepInvocation<'_>,
) -> Result<(ExitStatus, MemoryEvidence)> {
    let label = invocation.label;
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
        let command = step_command(step, roots, definition, environment, &invocation)?;
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
    invocation: &StepInvocation<'_>,
) -> Result<Command> {
    let label = invocation.label;
    let program = if step.program == "corepack" {
        corepack_executable()
    } else {
        step.program
    };
    let args = if is_common_frontend_fast(step) {
        frontend_fast_arguments(step, roots.frontend, roots.runner_frontend, roots.target)
    } else {
        render_args(step, roots, invocation)?
    };
    let mut command = if super::super::runtime_native::entry::is_native(program) {
        let mut input = invocation
            .runtime_input
            .ok_or("原生运行时步骤缺少显式输入")?
            .clone();
        input.output = roots.target.join(format!("runtime-{label}.json"));
        if args.get(1) != Some(&input.suite)
            || (input.suite != "homepage" && invocation.variant != input.concurrency.to_string())
        {
            return Err("原生运行时步骤的 suite 或并发与绑定输入不一致".into());
        }
        if input.backend != roots.backend
            || input.frontend != roots.frontend
            || input.runner_frontend != roots.runner_frontend
        {
            return Err("原生运行时步骤的源码目录与绑定输入不一致".into());
        }
        super::super::runtime_native::entry::command(&input)?
    } else {
        process_command(step, program, roots.backend, &args)?
    };
    configure_command(
        &mut command,
        step,
        roots,
        definition,
        environment,
        label,
        program,
    );
    if uses_historical_save_protocol(step) {
        super::save::bind_historical_protocol(&mut command, roots.target, invocation)?;
    }
    super::save::bind_private_config(&mut command, step, invocation);
    if let Some(pairing) = invocation.pairing
        && let Some(pair) = pairing.targeted_pair()
    {
        super::resource_gate::bind(
            &mut command,
            roots.backend,
            roots.target,
            invocation.label,
            pairing.arm,
            pair.arm(pairing.arm),
        )?;
    }
    Ok(command)
}

fn render_args(
    step: &StepDefinition,
    roots: StepRoots<'_>,
    invocation: &StepInvocation<'_>,
) -> Result<Vec<String>> {
    let target = roots.target.to_string_lossy();
    let frontend = if invocation
        .pairing
        .and_then(PairingMetadata::targeted_pair)
        .is_some()
    {
        std::borrow::Cow::Owned(super::super::support::child_path(roots.frontend))
    } else {
        roots.frontend.to_string_lossy()
    };
    let runner_frontend = roots.runner_frontend.to_string_lossy();
    let backend = roots.backend.to_string_lossy();
    let driver_root = crate::workspace::root_dir();
    let driver = driver_root.to_string_lossy();
    Ok(step_arguments(step, invocation.variant)?
        .iter()
        .map(|arg| {
            if arg == PAIRED_SAVE_OUTPUT {
                return super::save::paired_result_path(roots.target, invocation.label)
                    .to_string_lossy()
                    .into_owned();
            }
            if arg == RESOURCE_GATE_DECISION_TEMPLATE {
                return super::resource_gate::decision_path(roots.target, invocation.label)
                    .to_string_lossy()
                    .into_owned();
            }
            arg.replace("{driver}", &driver)
                .replace("{backend}", &backend)
                .replace("{target}", &target)
                .replace("{frontend}", &frontend)
                .replace("{runner-frontend}", &runner_frontend)
                .replace("{label}", invocation.label)
        })
        .collect())
}

fn process_command(
    step: &StepDefinition,
    program: &str,
    backend: &Path,
    args: &[String],
) -> Result<Command> {
    let mut command = Command::new(program);
    if step.program == "corepack" && args.first().is_some_and(|arg| arg == "pnpm") {
        command.arg("pnpm");
        if let Some(cache) = crate::process::pnpm_cache_argument(backend)? {
            command.arg(cache);
        }
        command.args(&args[1..]);
    } else {
        command.args(args);
        if is_common_frontend_fast(step)
            && let Some(cache) =
                ryframe_config::ToolSettings::load(backend)?.pnpm_cache_dir(backend)?
        {
            command.arg("--pnpm-cache-dir").arg(cache);
        }
    }
    command.stdin(Stdio::null());
    Ok(command)
}

fn configure_command(
    command: &mut Command,
    step: &StepDefinition,
    roots: StepRoots<'_>,
    definition: SuiteDefinition,
    environment: &BTreeMap<String, String>,
    label: &str,
    program: &str,
) {
    let target = roots.target.to_string_lossy();
    let frontend = roots.frontend.to_string_lossy();
    let runner_frontend = roots.runner_frontend.to_string_lossy();
    command
        .current_dir(match step.working_directory {
            WorkingDirectory::Backend => roots.backend,
            WorkingDirectory::Frontend => roots.frontend,
            WorkingDirectory::RunnerFrontend => roots.runner_frontend,
        })
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for key in definition.remove_environment {
        command.env_remove(key);
    }
    for (key, value) in environment {
        if super::super::runtime_native::entry::is_native(program) {
            if key.eq_ignore_ascii_case("APP_ENV") {
                continue;
            }
            if !super::super::runtime_native::entry::allowed_environment(key) {
                command.env_remove(key);
                continue;
            }
        }
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
        .env_remove("RYFRAME_DEVEX_TARGET_ROOT")
        .env_remove("RYFRAME_DEVEX_SAVE_CASE")
        .env_remove("RYFRAME_DEVEX_SAVE_RESULT_PATH")
        .env_remove("RYFRAME_FAST_CHECK_CACHE_ROOT");
}

#[cfg(test)]
#[path = "../../../tests/internal/devex_run_process.rs"]
mod tests;
