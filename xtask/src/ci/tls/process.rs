use std::{
    fs::File,
    io::Write,
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::{
    Result,
    process::{ChildGroup, stop_child},
};

const POLL_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Debug)]
pub(crate) enum ProcessOutcome {
    Exited(ExitStatus),
    TimedOut,
}

pub(crate) fn command_label(command: &Command) -> String {
    let mut parts = vec![command.get_program().to_string_lossy().into_owned()];
    parts.extend(
        command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned()),
    );
    parts.join(" ")
}

pub(crate) fn run_redirected(
    mut command: Command,
    log: &mut File,
    timeout: Duration,
) -> Result<ProcessOutcome> {
    let label = command_label(&command);
    writeln!(log, "command={label}")?;
    log.flush()?;
    let stdout = log.try_clone()?;
    let stderr = log.try_clone()?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));

    let mut child = ChildGroup::new()?
        .spawn(command)
        .map_err(|error| format!("无法启动 `{label}`：{error}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("等待 `{label}` 退出失败：{error}"))?
        {
            return Ok(ProcessOutcome::Exited(status));
        }
        if Instant::now() >= deadline {
            stop_child(&mut child)
                .map_err(|error| format!("超时后无法终止 `{label}` 的完整进程树：{error}"))?;
            return Ok(ProcessOutcome::TimedOut);
        }
        thread::sleep(POLL_INTERVAL);
    }
}
