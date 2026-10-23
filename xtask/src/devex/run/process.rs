use super::super::{
    memory::{self, MemoryEvidence},
    metadata::corepack_executable,
    model::{StepDefinition, SuiteDefinition, WorkingDirectory, step_arguments},
    support::{display_step, success_status},
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
    let target = roots.target.to_string_lossy();
    let frontend = roots.frontend.to_string_lossy();
    let runner_frontend = roots.runner_frontend.to_string_lossy();
    let backend = roots.backend.to_string_lossy();
    let driver_root = crate::workspace::root_dir();
    let driver = driver_root.to_string_lossy();
    let args = step_arguments(step, invocation.variant)?
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
    let mut command = if super::super::runtime_native::entry::is_native(program) {
        super::super::runtime_native::entry::command(&args, environment)?
    } else {
        let mut command = Command::new(program);
        command.args(&args).stdin(Stdio::null());
        command
    };
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
            if super::super::runtime_native::entry::private_environment(key) {
                command.env_remove(key);
                continue;
            }
            if key.eq_ignore_ascii_case("APP_ENV") {
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
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_step_keeps_only_app_env_from_project_protocol_environment() {
        let definition = super::super::super::model::DevexSuite::RuntimeApi
            .definition("10")
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let roots = StepRoots {
            backend: temp.path(),
            frontend: temp.path(),
            runner_frontend: temp.path(),
            target: temp.path(),
        };
        let environment = BTreeMap::from([
            ("APP_ENV".into(), "test".into()),
            ("APP_DATABASE_PASSWORD".into(), "must-not-leak".into()),
            ("VITE_API_URL".into(), "must-not-leak".into()),
            ("SNOWFLAKE_WORKER_ID".into(), "must-not-leak".into()),
            ("RYFRAME_DEVEX_CONCURRENCY".into(), "10".into()),
            ("RYFRAME_DEVEX_CACHE".into(), "warm".into()),
            ("RYFRAME_DEVEX_RUNTIME_INPUT_SHA256".into(), "a".repeat(64)),
            (
                "RYFRAME_DEVEX_DRIVER_FINGERPRINT".into(),
                format!("sha256:{}", "d".repeat(64)),
            ),
            (
                "RYFRAME_DEVEX_RUNNER_FRONTEND_FINGERPRINT".into(),
                format!("sha256:{}", "e".repeat(64)),
            ),
            ("RYFRAME_DEVEX_SOURCE_FINGERPRINTS".into(), "{}".into()),
            ("CARGO_BUILD_JOBS".into(), "1".into()),
        ]);
        let command = step_command(
            &definition.steps[0],
            roots,
            definition,
            &environment,
            &StepInvocation {
                variant: "10",
                label: "test",
            },
        )
        .unwrap();
        for (key, value) in command.get_envs() {
            let name = key.to_string_lossy();
            if super::super::super::runtime_native::entry::private_environment(&name) {
                assert!(value.is_none(), "{name}");
            }
        }
        assert!(command.get_envs().any(|(key, value)| key == "APP_ENV" && value == Some(std::ffi::OsStr::new("test"))));
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == "CARGO_BUILD_JOBS"
                    && value == Some(std::ffi::OsStr::new("1")))
        );
        assert!(
            command
                .get_envs()
                .all(|(_, value)| value != Some(std::ffi::OsStr::new("must-not-leak")))
        );
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![std::ffi::OsStr::new("__devex-runtime")]
        );
    }

    #[test]
    fn save_step_passes_validated_case_and_output_as_arguments() {
        let definition = super::super::super::model::DevexSuite::CargoDevSave
            .definition("worker-only")
            .unwrap();
        let roots = StepRoots {
            backend: Path::new("D:/measured-source"),
            frontend: Path::new("D:/frontend"),
            runner_frontend: Path::new("D:/runner-frontend"),
            target: Path::new("D:/sample-target"),
        };
        let invocation = StepInvocation {
            variant: "worker-only",
            label: "sample-001",
        };
        let command = step_command(
            &definition.steps[0],
            roots,
            definition,
            &BTreeMap::new(),
            &invocation,
        )
        .unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_ne!(roots.backend, roots.target);
        assert_eq!(command.get_current_dir(), Some(roots.backend));
        assert_eq!(
            args,
            [
                "xtask",
                "dev",
                "--measure-once",
                "--workspace-root",
                "D:/measured-source",
                "--case",
                "worker-only",
                "--output",
                "D:/sample-target/cargo-dev-save-result.json",
            ]
        );
        assert_eq!(
            command
                .get_envs()
                .find(|(key, _)| *key == "CARGO_TARGET_DIR")
                .and_then(|(_, value)| value),
            Some(roots.target.as_os_str()),
        );
        assert!(
            command
                .get_envs()
                .all(|(key, _)| key != "RYFRAME_DEVEX_SAVE_CASE"
                    && key != "RYFRAME_DEVEX_SAVE_RESULT_PATH")
        );
        let invalid = StepInvocation {
            variant: "unregistered",
            label: "sample-001",
        };
        assert!(
            step_command(
                &definition.steps[0],
                roots,
                definition,
                &BTreeMap::new(),
                &invalid
            )
            .is_err()
        );
    }
}
