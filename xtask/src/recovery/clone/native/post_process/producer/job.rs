//! 固定 Job 句柄与原创建身份共用同一回收边界。
use std::{
    path::Path,
    process::{ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;

use crate::{
    Result,
    process::{ChildGroup, ManagedChild, child_command, stop_child},
    recovery::clone::native::resources::common,
};

const CLEANUP_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct Job {
    child: ManagedChild,
    runtime: tokio::runtime::Runtime,
    pid: u32,
}

impl Job {
    pub(super) fn spawn(backend: &Path, argv: &[String], log: std::fs::File) -> Result<Self> {
        let mut command = child_command(argv.first().ok_or("生产者缺少已授权命令")?);
        command
            .args(argv.iter().skip(1))
            .current_dir(backend)
            .env_clear()
            .env("APP_ENV", "test")
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(log);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let child = {
            let _entered = runtime.enter();
            ChildGroup::new()?.spawn(command)?
        };
        let pid = child.id();
        Ok(Self {
            child,
            runtime,
            pid,
        })
    }

    pub(super) fn identity(&self) -> Result<Value> {
        let identity = common::process_identity(self.pid)?.ok_or("原生生产者缺少内核创建身份")?;
        self.check(&identity)?;
        Ok(identity)
    }

    pub(super) fn check(&self, identity: &Value) -> Result<()> {
        if common::process_identity(self.pid)?.as_ref() != Some(identity)
            || !self.child.member_pids()?.contains(&self.pid)
            || common::process_identity(self.pid)?.as_ref() != Some(identity)
        {
            return Err("原生生产者已退出、身份变化或脱离原受管进程树；保留证据，禁止重放".into());
        }
        Ok(())
    }

    pub(super) fn pipes(&mut self, session: bool) -> Result<super::pipes::Pipes> {
        let input = self
            .child
            .take_stdin_writer()?
            .ok_or("生产者缺少 stdin 管道")?;
        let output = self
            .child
            .take_stdout_reader()?
            .ok_or("生产者缺少 stdout 管道")?;
        super::pipes::Pipes::start(input, output, session)
    }

    pub(super) fn wait(&mut self, deadline: Instant) -> Result<ExitStatus> {
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("原生生产者未在期限内退出；结果未知，禁止重放".into());
            }
            thread::sleep(remaining.min(Duration::from_millis(10)));
        }
    }

    pub(super) fn finish(self, identity: Option<Value>) -> Result<()> {
        self.finish_until(identity, Instant::now() + CLEANUP_TIMEOUT)
    }

    pub(super) fn finish_until(self, identity: Option<Value>, deadline: Instant) -> Result<()> {
        let (send, receive) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("post-producer-cleanup".into())
            .spawn(move || {
                let Job {
                    mut child,
                    runtime,
                    pid,
                } = self;
                let result = close_owned(&mut child)
                    .and_then(|()| {
                        if let Some(identity) = identity
                            && common::process_identity(pid)?.as_ref() == Some(&identity)
                        {
                            return Err("生产者原创建身份在回收后仍存活".into());
                        }
                        Ok(())
                    })
                    .map_err(|error| error.to_string());
                // 即使显式回收失败，仍由原 Job 句柄执行析构，不根据 PID 接管其他进程。
                drop(child);
                drop(runtime);
                let _ = send.send(result);
            })?;
        match receive.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(error.into()),
            Err(_) => Err("原生生产者 Job 未在期限内确认回收；保留登记与回收线程，禁止重放".into()),
        }
    }
}

fn close_owned(child: &mut ManagedChild) -> Result<()> {
    let direct_exited = child.try_wait()?.is_some();
    if !direct_exited || !child.member_pids()?.is_empty() {
        stop_child(child)?;
    }
    if child.try_wait()?.is_none() || !child.member_pids()?.is_empty() {
        return Err("原生生产者或其 Job 成员尚未确认退出".into());
    }
    Ok(())
}
