//! 所有信号只使用显式 pidfd；不通过 Child::kill 或进程组回退。
use super::super::diagnostic::{self, Category, Outcome};
use crate::Result;
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal},
};
use std::{
    os::fd::OwnedFd,
    process::Child,
    time::{Duration, Instant},
};

pub(super) fn open_child(child: &Child) -> Result<OwnedFd> {
    // 必须先获取 fd，调用方此时尚未 wait/try_wait/reap 自己的 Child。
    pidfd_open(Pid::from_child(child), PidfdFlags::empty()).map_err(open_error)
}

pub(super) fn open(pid: u32) -> Result<Option<OwnedFd>> {
    let pid = Pid::from_raw(i32::try_from(pid)?).ok_or("缓存 PID 无效")?;
    match pidfd_open(pid, PidfdFlags::empty()) {
        Ok(fd) => Ok(Some(fd)),
        Err(rustix::io::Errno::SRCH) => Ok(None),
        Err(error) => Err(open_error(error)),
    }
}

pub(super) fn exited(fd: &OwnedFd, timeout: Duration) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout = Timespec::try_from(remaining)?;
        let mut descriptors = [PollFd::new(fd, PollFlags::IN)];
        match poll(&mut descriptors, Some(&timeout)) {
            Ok(0) => return Ok(false),
            Ok(_) if descriptors[0].revents().contains(PollFlags::IN) => return Ok(true),
            Ok(_) => {
                return Err(diagnostic::failure(
                    Category::ExitWaitFailed,
                    Outcome::Unknown,
                ));
            }
            Err(rustix::io::Errno::INTR) if Instant::now() < deadline => continue,
            Err(error) => {
                return Err(diagnostic::operating_system(
                    Category::ExitWaitFailed,
                    error,
                ));
            }
        }
    }
}

pub(super) fn signal(fd: &OwnedFd, signal: Signal) -> Result<bool> {
    if exited(fd, Duration::ZERO)? {
        return Ok(false);
    }
    match pidfd_send_signal(fd, signal) {
        Ok(()) => Ok(true),
        Err(rustix::io::Errno::SRCH) if exited(fd, Duration::ZERO)? => Ok(false),
        Err(error) => Err(diagnostic::operating_system(Category::SignalFailed, error)),
    }
}

pub(super) fn stop(fd: &OwnedFd) -> Result<bool> {
    let signalled = signal(fd, Signal::TERM)?;
    if !exited(fd, Duration::from_secs(5))? {
        signal(fd, Signal::KILL)?;
        if !exited(fd, Duration::from_secs(5))? {
            return Err(diagnostic::failure(
                Category::ExitWaitFailed,
                Outcome::Unknown,
            ));
        }
    }
    Ok(signalled)
}

fn open_error(error: rustix::io::Errno) -> Box<dyn std::error::Error> {
    let category = match error {
        rustix::io::Errno::NOSYS => Category::PidfdUnsupported,
        rustix::io::Errno::PERM | rustix::io::Errno::ACCESS => Category::PidfdPermissionDenied,
        _ => Category::PidfdOpenFailed,
    };
    diagnostic::operating_system(category, error)
}
