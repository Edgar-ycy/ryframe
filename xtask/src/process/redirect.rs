use std::{fs, path::Path, process::Stdio, time::Instant};

use crate::Result;

use super::{ChildGroup, CommandFailure, child_command, record_step, stop_child};

pub(crate) fn run_owned_with_env_redirected(
    dir: &Path,
    executable: &str,
    args: &[String],
    environment: &[(&str, &str)],
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<()> {
    let started = Instant::now();
    let stdout = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(stdout_path)
        .map_err(|error| format!("无法创建构建输出 {}：{error}", stdout_path.display()))?;
    let stderr = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(stderr_path)
        .map_err(|error| format!("无法创建构建日志 {}：{error}", stderr_path.display()))?;
    let mut command = child_command(executable);
    command
        .args(args)
        .envs(environment.iter().copied())
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let group = ChildGroup::new()?;
    let mut child = group.spawn(command)?;
    let status = child.wait();
    let elapsed = started.elapsed().as_secs_f64();
    let label = format!("{executable} {}", args.join(" "));
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            record_step(label, elapsed, false);
            return Err(error.into());
        }
    };
    if let Err(error) = stop_child(&mut child) {
        record_step(label, elapsed, false);
        return if status.success() {
            Err(error)
        } else {
            Err(CommandFailure::new(format!("命令退出且进程树回收失败：{error}"), status).into())
        };
    }
    record_step(label, elapsed, status.success());
    if status.success() {
        Ok(())
    } else {
        Err(CommandFailure::new(
            format!(
                "命令执行失败（{elapsed:.1}s）：{executable} {}",
                args.join(" ")
            ),
            status,
        )
        .into())
    }
}
