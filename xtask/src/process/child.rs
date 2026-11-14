use std::{
    io::{Read, Write},
    process::{Command, ExitStatus},
};

#[cfg(not(windows))]
use std::process::Child;
#[cfg(windows)]
use std::{thread, time::Duration};

use crate::Result;

#[cfg(unix)]
use super::prepare_process_group;

/// 开发任务的子进程工厂。每次 spawn 都创建独立的可终止进程树，避免一个构建代次
/// 借用整个 dev 会话的生命周期。
pub(crate) struct ChildGroup;

/// 一个直接子进程及其独立进程树所有权。
pub(crate) struct ManagedChild {
    #[cfg(windows)]
    child: tokio::process::Child,
    #[cfg(not(windows))]
    child: Child,
    #[cfg(windows)]
    group: Option<processkit::ProcessGroup>,
    #[cfg(unix)]
    process_group_id: u32,
}

impl ChildGroup {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self)
    }

    /// 接管命令所有权，确保同一 `Command` 不会重复叠加平台进程组配置。
    pub(crate) fn spawn(&self, command: Command) -> Result<ManagedChild> {
        spawn_managed(command)
    }
}

impl ManagedChild {
    /// 直接查询原 Job 的 PID 列表，成员身份由持有进程句柄的调用方核验。
    #[cfg(windows)]
    pub(crate) fn job_member_pids(&self) -> Result<Vec<u32>> {
        Ok(self.group().members()?)
    }

    /// 仅查询本句柄所拥有的整树成员，为持久监督器保存真实创建身份。
    pub(crate) fn member_pids(&self) -> Result<Vec<u32>> {
        #[cfg(windows)]
        {
            Ok(self
                .group()
                .members_info()?
                .into_iter()
                .map(|member| member.pid())
                .collect())
        }
        #[cfg(unix)]
        {
            let mut members = Vec::new();
            for entry in std::fs::read_dir("/proc")? {
                let entry = entry?;
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|v| v.parse::<u32>().ok())
                else {
                    continue;
                };
                let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                    continue;
                };
                let fields = stat
                    .rsplit_once(')')
                    .map(|(_, v)| v.split_whitespace().collect::<Vec<_>>())
                    .unwrap_or_default();
                if fields.get(2).and_then(|v| v.parse::<u32>().ok()) == Some(self.process_group_id)
                {
                    members.push(pid);
                }
            }
            Ok(members)
        }
    }
    pub(crate) fn id(&self) -> u32 {
        #[cfg(windows)]
        {
            self.child
                .id()
                .expect("ManagedChild 在 wait 完成前仍应保留进程 ID")
        }
        #[cfg(not(windows))]
        {
            self.child.id()
        }
    }

    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// 保持现有同步 xtask 调用合同，不要求调用方创建 Tokio runtime。
    pub(crate) fn wait(&mut self) -> std::io::Result<ExitStatus> {
        #[cfg(windows)]
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            thread::sleep(Duration::from_millis(10));
        }
        #[cfg(not(windows))]
        {
            self.child.wait()
        }
    }

    #[cfg(not(windows))]
    pub(crate) fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill()
    }

    /// 把管道转换成可在线程中同步读取的所有权对象；Windows 不轮询异步句柄，
    /// 因而该路径同样不要求 Tokio runtime。
    pub(crate) fn take_stdout_reader(&mut self) -> Result<Option<Box<dyn Read + Send>>> {
        let Some(stdout) = self.child.stdout.take() else {
            return Ok(None);
        };
        #[cfg(windows)]
        {
            let handle = stdout.into_owned_handle()?;
            Ok(Some(Box::new(std::fs::File::from(handle))))
        }
        #[cfg(not(windows))]
        {
            Ok(Some(Box::new(stdout)))
        }
    }

    /// 仅移交已有管道；调用方核验 ready 后才写一次授权，并负责 flush 和关闭。
    pub(crate) fn take_stdin_writer(&mut self) -> Result<Option<Box<dyn Write + Send>>> {
        let Some(stdin) = self.child.stdin.take() else {
            return Ok(None);
        };
        #[cfg(windows)]
        {
            let handle = stdin.into_owned_handle()?;
            Ok(Some(Box::new(std::fs::File::from(handle))))
        }
        #[cfg(not(windows))]
        {
            Ok(Some(Box::new(stdin)))
        }
    }

    /// 与 stdout 相同地把 stderr 管道交给同步读取线程，供可取消的捕获命令使用。
    pub(crate) fn take_stderr_reader(&mut self) -> Result<Option<Box<dyn Read + Send>>> {
        let Some(stderr) = self.child.stderr.take() else {
            return Ok(None);
        };
        #[cfg(windows)]
        {
            let handle = stderr.into_owned_handle()?;
            Ok(Some(Box::new(std::fs::File::from(handle))))
        }
        #[cfg(not(windows))]
        {
            Ok(Some(Box::new(stderr)))
        }
    }

    #[cfg(windows)]
    pub(crate) fn active_process_count(&self) -> Result<Option<u32>> {
        let active = self.job_stats()?.active_process_count;
        Ok(Some(u32::try_from(active).map_err(|_| {
            format!("Windows Job Object 活跃进程数超出 u32 范围：{active}")
        })?))
    }

    #[cfg(not(windows))]
    pub(crate) const fn active_process_count(&self) -> Result<Option<u32>> {
        Ok(None)
    }

    #[cfg(windows)]
    pub(crate) fn job_stats(&self) -> Result<processkit::ProcessGroupStats> {
        Ok(self.group().stats()?)
    }

    #[cfg(windows)]
    pub(super) fn terminate_tree(&self) -> Result<()> {
        self.group().kill_all()?;
        Ok(())
    }

    #[cfg(windows)]
    pub(super) fn member_handles(&self) -> Result<super::windows_members::MemberHandles> {
        super::windows_members::MemberHandles::capture(self.group())
    }

    #[cfg(windows)]
    fn group(&self) -> &processkit::ProcessGroup {
        self.group
            .as_ref()
            .expect("ManagedChild 释放前必须持有 Windows Job Object")
    }

    #[cfg(unix)]
    pub(super) const fn process_group_id(&self) -> u32 {
        self.process_group_id
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            // 先关闭带 KILL_ON_JOB_CLOSE 的 Job 句柄，再等待直接子进程，保持原有
            // 析构顺序；即使显式 kill_all 失败也不会卡在仍运行的直接子进程上。
            drop(self.group.take());
        }
        #[cfg(unix)]
        {
            let _ = super::signal_process_group(
                self.process_group_id,
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = self.child.kill();
        }
        let _ = self.wait();
    }
}

#[cfg(windows)]
fn spawn_managed(command: Command) -> Result<ManagedChild> {
    let group = processkit::ProcessGroup::new()
        .map_err(|error| format!("无法创建 Windows 子进程 Job Object：{error}"))?;
    let child = group
        .spawn(tokio::process::Command::from(command))
        .map_err(|error| format!("无法启动 Windows Job Object 子进程：{error}"))?;
    Ok(ManagedChild {
        child,
        group: Some(group),
    })
}

#[cfg(unix)]
fn spawn_managed(mut command: Command) -> Result<ManagedChild> {
    prepare_process_group(&mut command);
    let child = command.spawn()?;
    let process_group_id = child.id();
    Ok(ManagedChild {
        child,
        process_group_id,
    })
}

#[cfg(not(any(unix, windows)))]
fn spawn_managed(mut command: Command) -> Result<ManagedChild> {
    let child = command.spawn()?;
    Ok(ManagedChild { child })
}
