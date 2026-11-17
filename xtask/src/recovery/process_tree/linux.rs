//! 原 supervisor 不先成为进程组长；保留 pidfd 到整段成员关闭之后才 reap。
use super::check;
use crate::{Result, recovery::clone::native::resources::common};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal},
};
use serde_json::Value;
use std::time::{Duration, Instant};
use std::{
    fs,
    os::fd::OwnedFd,
    process::{Child, Command},
};

struct Member {
    identity: Value,
    fd: OwnedFd,
}
pub(in crate::recovery) struct Group {
    child: Child,
    supervisor: Value,
    members: Vec<Member>,
}
pub(in crate::recovery) fn supported() -> Result<()> {
    Ok(())
}

impl Group {
    pub(in crate::recovery) fn spawn(
        mut command: Command,
        session_timeout: Duration,
    ) -> Result<Self> {
        // 此处不能 process_group(0)，supervisor 必须先安全 setsid。
        let mut child = command.spawn()?;
        let fd = match pidfd_open(Pid::from_child(&child), PidfdFlags::empty()) {
            Ok(fd) => fd,
            Err(error) => {
                // 此时 supervisor 尚不能越过 monitor ready 门；仅回收自有 Child 句柄。
                let killed = child.kill();
                let waited = child.wait();
                killed?;
                waited?;
                return Err(error.into());
            }
        };
        let prepared = (|| {
            let supervisor =
                common::process_identity(child.id())?.ok_or("RustFS supervisor 创建身份缺失")?;
            let deadline = Instant::now() + session_timeout;
            while group_of(child.id())? != (child.id(), child.id()) {
                check(
                    !exited(&fd)? && Instant::now() < deadline,
                    "RustFS supervisor 未建立独立 session",
                )?;
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(supervisor)
        })();
        match prepared {
            Ok(supervisor) => Ok(Self {
                child,
                supervisor: supervisor.clone(),
                members: vec![Member {
                    identity: supervisor,
                    fd,
                }],
            }),
            Err(error) => {
                let killed = signal(&fd, Signal::KILL);
                let waited = child.wait();
                killed?;
                waited?;
                Err(error)
            }
        }
    }
    pub(in crate::recovery) fn identity(&self) -> &Value {
        &self.supervisor
    }
    pub(in crate::recovery) fn members(&self) -> Vec<Value> {
        self.members.iter().map(|m| m.identity.clone()).collect()
    }
    pub(in crate::recovery) fn capture(&mut self) -> Result<()> {
        let group = self.child.id();
        for entry in fs::read_dir("/proc")? {
            let entry = entry?;
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|v| v.parse::<u32>().ok())
            else {
                continue;
            };
            let observed = match group_of(pid) {
                Ok(value) => value,
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            };
            if observed.0 != group && observed.1 != group {
                continue;
            }
            check(
                observed == (group, group),
                "RustFS 成员离开原 session 或进程组",
            )?;
            if self.members.iter().any(|m| m.identity["pid"] == pid) {
                continue;
            }
            let descriptor = Pid::from_raw(i32::try_from(pid)?).ok_or("成员 PID 无效")?;
            let fd = match pidfd_open(descriptor, PidfdFlags::empty()) {
                Ok(fd) => fd,
                Err(rustix::io::Errno::SRCH) => continue,
                Err(error) => return Err(error.into()),
            };
            if exited(&fd)? {
                continue;
            }
            let identity = common::process_identity(pid)?.ok_or("RustFS 成员创建身份未知")?;
            check(
                group_of(pid)? == (group, group)
                    && !exited(&fd)?
                    && common::process_identity(pid)?.as_ref() == Some(&identity),
                "RustFS 成员采集期间身份变化",
            )?;
            self.members.push(Member { identity, fd });
        }
        for member in &self.members {
            if !exited(&member.fd)? {
                let pid = u32::try_from(member.identity["pid"].as_u64().ok_or("成员 PID 无效")?)?;
                check(
                    group_of(pid)? == (group, group)
                        && common::process_identity(pid)?.as_ref() == Some(&member.identity),
                    "已登记 RustFS 成员身份或 session 变化",
                )?;
            }
        }
        Ok(())
    }
    pub(in crate::recovery) fn supervisor_exited(&self) -> Result<bool> {
        exited(&self.members[0].fd)
    }
    pub(in crate::recovery) fn close(&mut self) -> Result<()> {
        let mut failure = self.capture().err();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Err(error) = self.capture() {
                failure.get_or_insert(error);
            }
            let mut all = true;
            for member in &self.members {
                if !exited(&member.fd)? {
                    all = false;
                    signal(&member.fd, Signal::KILL)?;
                }
            }
            if all {
                break;
            }
            check(Instant::now() < deadline, "RustFS pidfd 完整成员未关闭")?;
            std::thread::sleep(Duration::from_millis(20));
        }
        // group ID 仍由未 reap 的原子进程占有；所有 fd 退出后才 wait。
        self.child.wait()?;
        failure.map_or(Ok(()), Err)
    }
}

pub(in crate::recovery) fn exited(fd: &OwnedFd) -> Result<bool> {
    let zero = Timespec::try_from(Duration::ZERO)?;
    let mut descriptors = [PollFd::new(fd, PollFlags::IN)];
    loop {
        match poll(&mut descriptors, Some(&zero)) {
            Ok(0) => return Ok(false),
            Ok(_) if descriptors[0].revents().contains(PollFlags::IN) => return Ok(true),
            Err(rustix::io::Errno::INTR) => continue,
            _ => return Err("RustFS pidfd 退出观察失败".into()),
        }
    }
}
pub(in crate::recovery) fn signal(fd: &OwnedFd, signal: Signal) -> Result<()> {
    if !exited(fd)? {
        match pidfd_send_signal(fd, signal) {
            Ok(()) => (),
            Err(rustix::io::Errno::SRCH) if exited(fd)? => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
fn group_of(pid: u32) -> Result<(u32, u32)> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let fields = stat
        .rsplit_once(')')
        .ok_or("RustFS /proc stat 无效")?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    let parse = |index: usize| -> Result<u32> {
        fields
            .get(index)
            .ok_or("RustFS /proc 字段缺失")?
            .parse::<u32>()
            .map_err(Into::into)
    };
    Ok((parse(2)?, parse(3)?))
}
