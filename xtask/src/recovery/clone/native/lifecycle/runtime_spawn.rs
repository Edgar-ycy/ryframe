//! 启动旧版复制运行的单个产品，调用方持有本次创建的进程句柄。

use crate::{
    Result,
    recovery::runtime::start::actor::{system_environment, system_environment_key},
};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    path::Path,
    process::{Command, Stdio},
};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(super) use windows::{OwnedChild, check_pending, spawn};

pub(in crate::recovery) const EXEC_ENTRY: &str = "__clone-runtime-exec";

pub(super) struct Launch<'a> {
    pub(super) backend: &'a Path,
    pub(super) artifact: &'a Path,
    pub(super) configuration: &'a Path,
    pub(super) environment: &'a BTreeMap<String, String>,
    pub(super) log: &'a Path,
}

#[cfg(not(windows))]
pub(super) struct OwnedChild(std::process::Child);

#[cfg(not(windows))]
impl OwnedChild {
    pub(super) fn id(&self) -> u32 {
        self.0.id()
    }

    pub(super) fn is_exited(&mut self) -> Result<bool> {
        Ok(self.0.try_wait()?.is_some())
    }

    pub(super) fn kill(&mut self) -> Result<()> {
        Ok(self.0.kill()?)
    }
}

#[cfg(not(windows))]
pub(super) fn spawn(launch: &Launch<'_>) -> Result<OwnedChild> {
    let log = new_log(launch.log)?;
    let mut command = platform_command(launch.artifact, launch.configuration)?;
    prepare_command(&mut command, launch.backend, launch.environment, &log)?;
    Ok(OwnedChild(command.spawn()?))
}

fn new_log(path: &Path) -> Result<File> {
    Ok(OpenOptions::new().write(true).create_new(true).open(path)?)
}

fn prepare_command(
    command: &mut Command,
    backend: &Path,
    environment: &BTreeMap<String, String>,
    log: &File,
) -> Result<()> {
    command
        .current_dir(backend)
        .env_clear()
        .envs(system_environment().filter(|(key, _)| {
            !environment
                .keys()
                .any(|name| name.eq_ignore_ascii_case(&key.to_string_lossy()))
        }))
        .envs(
            environment
                .iter()
                .filter(|(key, _)| system_environment_key(key)),
        )
        .env("APP_ENV", "test")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log.try_clone()?);
    Ok(())
}

#[cfg(windows)]
pub(in crate::recovery) fn run_exec(args: &[String]) -> Result<()> {
    windows::run_exec(args)
}

#[cfg(target_os = "linux")]
fn platform_command(artifact: &Path, configuration: &Path) -> Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg(EXEC_ENTRY)
        .arg(artifact)
        .arg("--config-dir")
        .arg(configuration);
    Ok(command)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn platform_command(_artifact: &Path, _configuration: &Path) -> Result<Command> {
    Err("当前平台不支持旧版复制运行的单进程启动".into())
}

#[cfg(target_os = "linux")]
pub(in crate::recovery) fn run_exec(args: &[String]) -> Result<()> {
    use crate::local_test_path::{LocalTestPathKind, validate_absolute_path};
    use crate::recovery::runtime::start::actor::restrict_environment;
    use std::os::unix::process::CommandExt;

    let [artifact, option, configuration] = args else {
        return Err("旧版复制运行的启动跳板参数数量无效".into());
    };
    if option != "--config-dir" {
        return Err("旧版复制运行的启动跳板缺少明确的配置目录参数".into());
    }
    if std::env::var("APP_ENV").as_deref() != Ok("test") {
        return Err("旧版复制运行的启动跳板要求 APP_ENV=test".into());
    }
    let artifact = validate_absolute_path(Path::new(artifact), LocalTestPathKind::ExistingFile)?;
    let configuration = validate_absolute_path(
        Path::new(configuration),
        LocalTestPathKind::ExistingDirectory,
    )?;

    // 新会话先建立，产品随后占用同一个 PID 并继承已限制的环境和日志。
    nix::unistd::setsid()?;
    let mut command = Command::new(artifact);
    restrict_environment(&mut command, true);
    Err(command.arg("--config-dir").arg(configuration).exec().into())
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(in crate::recovery) fn run_exec(_args: &[String]) -> Result<()> {
    Err("当前平台不支持旧版复制运行的 Linux 启动跳板".into())
}
