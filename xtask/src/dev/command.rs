use std::{
    path::Path,
    process::{Command, Stdio},
};

pub(crate) fn api_command(
    root: &Path,
    binary: &Path,
    api_port: u16,
    worker_port: u16,
    worker_id: u16,
    probe: bool,
) -> Command {
    let mut command = Command::new(binary);
    command
        .env("APP_ENV", "dev")
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
    port: u16,
    worker_id: u16,
    probe: bool,
) -> Command {
    let mut command = Command::new(binary);
    command
        .env("APP_ENV", "dev")
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
