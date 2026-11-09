//! 旧恢复产品的单进程停止；信号和退出观察始终绑定原 pidfd。
use crate::{
    Result,
    recovery::{clone::native::resources::common, process_tree::exited},
};
use rustix::{
    io::Errno,
    process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal},
};
use serde_json::Value;
use std::{
    os::fd::OwnedFd,
    thread,
    time::{Duration, Instant},
};

pub(super) fn stop_process(pid: u32, expected: &Value) -> Result<bool> {
    let target = Pid::from_raw(i32::try_from(pid)?).ok_or("登记进程 PID 无效")?;
    let fd = match pidfd_open(target, PidfdFlags::empty()) {
        Ok(fd) => fd,
        Err(Errno::SRCH) => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if exited(&fd)? {
        return Ok(false);
    }
    let actual = match common::process_identity(pid) {
        Ok(actual) => actual,
        Err(_) if exited(&fd)? => return Ok(false),
        Err(error) => return Err(error),
    };
    if actual.as_ref().is_some_and(|actual| actual != expected) {
        return Err("登记 PID 已复用或创建身份不同，未发送信号".into());
    }
    if exited(&fd)? {
        return Ok(false);
    }
    if actual.is_none() {
        return Err("原 pidfd 尚未确认退出且无法取得登记进程身份，未发送信号".into());
    }
    stop_with(|signal| send(&fd, signal), |timeout| wait(&fd, timeout))
}

fn send(fd: &OwnedFd, signal: Signal) -> Result<bool> {
    match pidfd_send_signal(fd, signal) {
        Ok(()) => Ok(true),
        Err(Errno::SRCH) if exited(fd)? => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn wait(fd: &OwnedFd, timeout: Duration) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        if exited(fd)? {
            return Ok(true);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        thread::sleep(remaining.min(Duration::from_millis(20)));
    }
}

fn stop_with(
    mut send: impl FnMut(Signal) -> Result<bool>,
    mut wait: impl FnMut(Duration) -> Result<bool>,
) -> Result<bool> {
    let timeout = Duration::from_secs(5);
    if !send(Signal::TERM)? {
        return Ok(false);
    }
    if wait(timeout)? {
        return Ok(true);
    }
    if !send(Signal::KILL)? {
        return Ok(false);
    }
    if !wait(timeout)? {
        return Err("原登记进程在升级停止后未在期限内退出".into());
    }
    Ok(true)
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/fixture_control/services/lifecycle/tree_linux.rs"]
mod tests;
