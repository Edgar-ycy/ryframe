use std::{
    path::Path,
    process::{Command, Stdio},
};

#[derive(Clone, Copy)]
pub(crate) struct RuntimeInputPaths<'a> {
    config_dir: &'a Path,
}
impl<'a> RuntimeInputPaths<'a> {
    pub(crate) const fn new(config_dir: &'a Path) -> Self {
        Self { config_dir }
    }
}

pub(crate) fn api_command(
    root: &Path,
    binary: &Path,
    runtime: RuntimeInputPaths<'_>,
    probe: bool,
) -> Command {
    role_command(root, binary, runtime, probe)
}
pub(crate) fn worker_command(
    root: &Path,
    binary: &Path,
    runtime: RuntimeInputPaths<'_>,
    probe: bool,
) -> Command {
    role_command(root, binary, runtime, probe)
}
fn role_command(
    root: &Path,
    binary: &Path,
    runtime: RuntimeInputPaths<'_>,
    probe: bool,
) -> Command {
    let mut command = Command::new(binary);
    command
        .env("APP_ENV", "dev")
        .arg("--config-dir")
        .arg(runtime.config_dir)
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if probe {
        command.arg("--probe");
    }
    command
}
