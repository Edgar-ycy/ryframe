use std::{
    path::Path,
    process::{Command, Stdio},
};

#[derive(Clone, Copy)]
pub(crate) struct RuntimeInputPaths<'a> {
    config_dir: &'a Path,
    locales_dir: &'a Path,
}

impl<'a> RuntimeInputPaths<'a> {
    pub(crate) const fn new(config_dir: &'a Path, locales_dir: &'a Path) -> Self {
        Self {
            config_dir,
            locales_dir,
        }
    }
}

pub(crate) fn api_command(
    root: &Path,
    binary: &Path,
    runtime_inputs: RuntimeInputPaths<'_>,
    api_port: u16,
    worker_port: u16,
    worker_id: u16,
    probe: bool,
) -> Command {
    let mut command = Command::new(binary);
    command
        .env("APP_ENV", "dev")
        .env("APP_CONFIG_DIR", runtime_inputs.config_dir)
        .env("APP_LOCALES_DIR", runtime_inputs.locales_dir)
        .env("APP_DATABASE_MIGRATION_MODE", "verify")
        .env("APP_JOBS_MODE", "external")
        .env("APP_APP_PORT", api_port.to_string())
        .env("APP_JOBS_HEALTH_PORT", worker_port.to_string())
        .env("SNOWFLAKE_WORKER_ID", worker_id.to_string())
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if probe {
        command.arg("--probe");
        command
            .env("APP_APP_HOST", "127.0.0.1")
            .env("APP_TELEMETRY_ENABLED", "false")
            .env("APP_LOGGER_OUTPUT", "stdout");
    }
    command
}

pub(crate) fn worker_command(
    root: &Path,
    binary: &Path,
    runtime_inputs: RuntimeInputPaths<'_>,
    port: u16,
    worker_id: u16,
    probe: bool,
) -> Command {
    let mut command = Command::new(binary);
    command
        .env("APP_ENV", "dev")
        .env("APP_CONFIG_DIR", runtime_inputs.config_dir)
        .env("APP_LOCALES_DIR", runtime_inputs.locales_dir)
        .env("APP_DATABASE_MIGRATION_MODE", "verify")
        .env("APP_JOBS_MODE", "external")
        .env("APP_JOBS_HEALTH_PORT", port.to_string())
        .env("SNOWFLAKE_WORKER_ID", worker_id.to_string())
        .env("APP_JOBS_WORKER_ID", "ryframe-dev-worker")
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if probe {
        command.arg("--probe");
        command
            .env("APP_JOBS_WORKER_ID", "ryframe-dev-probe")
            .env("APP_JOBS_HEALTH_HOST", "127.0.0.1")
            .env("APP_TELEMETRY_ENABLED", "false")
            .env("APP_LOGGER_OUTPUT", "stdout");
    }
    command
}
