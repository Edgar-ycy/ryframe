//! Job 成员和实际句柄只归本 monitor；产品继承 supervisor 所在 Job。
use super::check;
use crate::{
    Result,
    process::{ChildGroup, ManagedChild, stop_child},
    recovery::clone::native::lifecycle::identity,
};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::time::{Duration, Instant};
use winsafe::{HPROCESS, co, guard::CloseHandleGuard, prelude::kernel_Hprocess};

struct Member {
    pid: u32,
    identity: Value,
    handle: CloseHandleGuard<HPROCESS>,
}
pub(in crate::recovery) struct Group {
    child: Option<ManagedChild>,
    supervisor: Value,
    members: Vec<Member>,
}

pub(in crate::recovery) fn supported() -> Result<()> {
    Ok(())
}

impl Group {
    pub(in crate::recovery) fn spawn(
        command: std::process::Command,
        _session_timeout: Duration,
    ) -> Result<Self> {
        let mut child = ChildGroup::new()?.spawn(command)?;
        let supervisor = match capture_supervisor(&mut child) {
            Ok(member) => member,
            Err(error) => return reject_child(child, error),
        };
        let mut group = Self {
            child: Some(child),
            supervisor: supervisor.identity.clone(),
            members: vec![supervisor],
        };
        let captured = group
            .capture()
            .and_then(|()| check(!group.supervisor_exited()?, "原 supervisor 采集期间退出"));
        if let Err(error) = captured {
            // 观察失败也显式执行自有 Job 的停止，不提前依赖隐式析构作成功证明。
            let closed = group.close();
            closed?;
            return Err(error);
        }
        Ok(group)
    }
    pub(in crate::recovery) fn identity(&self) -> &Value {
        &self.supervisor
    }
    pub(in crate::recovery) fn members(&self) -> Vec<Value> {
        self.members.iter().map(|m| m.identity.clone()).collect()
    }
    pub(in crate::recovery) fn capture(&mut self) -> Result<()> {
        let child = self.child.as_ref().ok_or("原 Job 句柄已经释放")?;
        let pids = child.job_member_pids()?;
        for member in &self.members {
            if !pids.contains(&member.pid) {
                check(exited(&member.handle, 0)?, "存活的原成员不在 Job 中")?;
            }
        }
        for pid in pids {
            if let Some(member) = self.members.iter().rev().find(|member| member.pid == pid)
                && (observe_member(child, &member.handle, pid, Some(&member.identity))?.is_some()
                    || !child.job_member_pids()?.contains(&pid))
            {
                continue;
            }
            if let Some(member) = capture_member(child, pid)? {
                self.members.push(member);
            }
        }
        Ok(())
    }
    pub(in crate::recovery) fn supervisor_exited(&self) -> Result<bool> {
        let member = self
            .members
            .iter()
            .find(|m| m.identity == self.supervisor)
            .ok_or("缺少 supervisor 句柄")?;
        exited(&member.handle, 0)
    }
    pub(in crate::recovery) fn close(&mut self) -> Result<()> {
        let capture = self.capture();
        let stopped = stop_child(self.child.as_mut().ok_or("原 Job 句柄已经释放")?);
        if stopped.is_err() {
            // stop_child 自身的首轮成员读取也可能失败；释放自有 KILL_ON_JOB_CLOSE 句柄仍执行。
            drop(self.child.take());
        }
        // 即使采集失败，也必须执行已持有 Job 的整树停止和原成员句柄等待。
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut all_exited = true;
            for member in &self.members {
                if !exited(&member.handle, 0)? {
                    all_exited = false;
                }
            }
            let empty = match self.child.as_ref() {
                Some(child) => child.active_process_count()? == Some(0),
                None => true,
            };
            if all_exited && empty {
                stopped?;
                return capture;
            }
            check(
                Instant::now() < deadline,
                "Job 已知成员句柄或完整计数仍未关闭",
            )?;
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn capture_supervisor(child: &mut ManagedChild) -> Result<Member> {
    let pid = child.id();
    check(child.try_wait()?.is_none(), "原 supervisor 已退出")?;
    let member = capture_member(child, pid)?.ok_or("RustFS supervisor 创建身份缺失")?;
    // 用实际直接子进程的持有句柄排除采集期间 PID 被另一个 Job 成员复用。
    check(child.try_wait()?.is_none(), "原 supervisor 采集期间退出")?;
    Ok(member)
}

fn capture_member(child: &ManagedChild, pid: u32) -> Result<Option<Member>> {
    let handle = HPROCESS::OpenProcess(
        co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
        false,
        pid,
    )?;
    Ok(
        observe_member(child, &handle, pid, None)?.map(|identity| Member {
            pid,
            identity,
            handle,
        }),
    )
}

fn observe_member(
    child: &ManagedChild,
    handle: &HPROCESS,
    pid: u32,
    expected: Option<&Value>,
) -> Result<Option<Value>> {
    if exited(handle, 0)? {
        return Ok(None);
    }
    let Some(identity) = confirm_snapshot(handle, identity::windows_snapshot(handle, pid))? else {
        return Ok(None);
    };
    let in_job = child.job_member_pids()?.contains(&pid);
    // 成员可在重读 Job 时退出；仍活跃的句柄必须和原创建身份及 Job 成员表一致。
    if exited(handle, 0)? {
        return Ok(None);
    }
    check(
        in_job && expected.is_none_or(|expected| *expected == identity),
        "Job 成员句柄创建身份或归属变化",
    )?;
    Ok(Some(identity))
}

fn confirm_snapshot(handle: &HPROCESS, snapshot: Result<Value>) -> Result<Option<Value>> {
    // 身份读取失败只能由同一句柄的退出状态消解；正常观察不延长等待。
    let timeout = if snapshot.is_err() { 1000 } else { 0 };
    identity::confirm_live_observation(snapshot, exited(handle, timeout))
}

fn exited(handle: &HPROCESS, timeout_ms: u32) -> Result<bool> {
    match handle.WaitForSingleObject(Some(timeout_ms))? {
        co::WAIT::OBJECT_0 => Ok(true),
        co::WAIT::TIMEOUT => Ok(false),
        _ => Err("Windows Job 成员句柄返回未知等待状态".into()),
    }
}

fn reject_child(mut child: ManagedChild, error: Box<dyn std::error::Error>) -> Result<Group> {
    let stopped = stop_child(&mut child);
    let empty = child.active_process_count();
    // ManagedChild::drop 先关闭本句柄的 KILL_ON_JOB_CLOSE Job，再 wait；
    // 即使 stop_child 的成员采集失败也不能跳过精确所有权回收。
    drop(child);
    stopped?;
    check(empty? == Some(0), "RustFS 创建失败后原 Job 成员退出未确认")?;
    Err(error)
}

#[cfg(test)]
#[test]
fn rustfs_windows_job_capture_skips_exited_member_and_rejects_live_drift() {
    use std::{
        io::Read,
        process::{Command, Stdio},
    };

    const PROBE: &str = "RYFRAME_TEST_RUSTFS_JOB_CAPTURE_CHILD";
    if std::env::var_os(PROBE).is_some() {
        std::io::stdin().read_exact(&mut [0]).unwrap();
        return;
    }
    let test = format!(
        "{}::rustfs_windows_job_capture_skips_exited_member_and_rejects_live_drift",
        module_path!().split_once("::").unwrap().1
    );
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", &test])
        .env(PROBE, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null());
    let mut child = ChildGroup::new().unwrap().spawn(command).unwrap();
    let pid = child.id();
    let observed = identity::process(pid).unwrap().unwrap();
    let captured = capture_member(&child, pid).unwrap().unwrap();
    assert_eq!(captured.identity, observed);

    let mut replaced = observed.clone();
    replaced["started"] = json!("1");
    assert!(observe_member(&child, &captured.handle, pid, Some(&replaced)).is_err());
    assert!(capture_member(&child, std::process::id()).is_err());
    let unopened = capture_member(&child, 0).err().unwrap();
    assert_eq!(
        unopened.downcast_ref::<co::ERROR>(),
        Some(&co::ERROR::INVALID_PARAMETER)
    );
    let denied = || Err(co::ERROR::ACCESS_DENIED.into());
    let error = confirm_snapshot(&captured.handle, denied()).unwrap_err();
    assert_eq!(
        error.downcast_ref::<co::ERROR>(),
        Some(&co::ERROR::ACCESS_DENIED)
    );

    child
        .take_stdin_writer()
        .unwrap()
        .unwrap()
        .write_all(b"x")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "测试 Job 子进程未退出");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(exited(&captured.handle, 0).unwrap());
    assert!(
        confirm_snapshot(&captured.handle, denied())
            .unwrap()
            .is_none()
    );
    assert!(
        observe_member(&child, &captured.handle, pid, Some(&observed))
            .unwrap()
            .is_none()
    );
}
