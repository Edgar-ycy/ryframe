//! Job 成员和实际句柄只归本 monitor；产品继承 supervisor 所在 Job。
use super::*;
use crate::{
    process::{ChildGroup, ManagedChild, stop_child},
    recovery::clone::native::lifecycle::identity,
};
use winsafe::{HPROCESS, co, guard::CloseHandleGuard, prelude::kernel_Hprocess};

struct Member {
    identity: Value,
    handle: CloseHandleGuard<HPROCESS>,
}
pub(super) struct Group {
    child: Option<ManagedChild>,
    supervisor: Value,
    members: Vec<Member>,
}

pub(super) fn supported() -> Result<()> {
    Ok(())
}

impl Group {
    pub(super) fn spawn(command: std::process::Command) -> Result<Self> {
        let child = ChildGroup::new()?.spawn(command)?;
        let supervisor = match common::process_identity(child.id()) {
            Ok(Some(identity)) => identity,
            Ok(None) => return reject_child(child, "RustFS supervisor 创建身份缺失".into()),
            Err(error) => return reject_child(child, error),
        };
        let mut group = Self {
            child: Some(child),
            supervisor,
            members: Vec::new(),
        };
        let captured = group.capture().and_then(|_| {
            check(
                group.members().contains(&group.supervisor),
                "Job 未包含原 supervisor",
            )
        });
        if let Err(error) = captured {
            // 观察失败也显式执行自有 Job 的停止，不提前依赖隐式析构作成功证明。
            let closed = group.close();
            closed?;
            return Err(error);
        }
        Ok(group)
    }
    pub(super) fn identity(&self) -> &Value {
        &self.supervisor
    }
    pub(super) fn members(&self) -> Vec<Value> {
        self.members.iter().map(|m| m.identity.clone()).collect()
    }
    pub(super) fn capture(&mut self) -> Result<()> {
        let child = self.child.as_ref().ok_or("原 Job 句柄已经释放")?;
        for pid in child.member_pids()? {
            let Some(identity) = common::process_identity(pid)? else {
                continue;
            };
            if self
                .members
                .iter()
                .any(|member| member.identity == identity)
            {
                continue;
            }
            if let Some(member) = capture_member(child, pid, &identity)? {
                self.members.push(member);
            }
        }
        Ok(())
    }
    pub(super) fn supervisor_exited(&self) -> Result<bool> {
        let member = self
            .members
            .iter()
            .find(|m| m.identity == self.supervisor)
            .ok_or("缺少 supervisor 句柄")?;
        Ok(member.handle.WaitForSingleObject(Some(0))? == co::WAIT::OBJECT_0)
    }
    pub(super) fn close(&mut self) -> Result<()> {
        let capture = self.capture();
        let stopped = stop_child(self.child.as_mut().ok_or("原 Job 句柄已经释放")?);
        if stopped.is_err() {
            // stop_child 自身的首轮成员读取也可能失败；释放自有 KILL_ON_JOB_CLOSE 句柄仍执行。
            drop(self.child.take());
        }
        // 即使采集失败，也必须执行已持有 Job 的整树停止和原成员句柄等待。
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut exited = true;
            for member in &self.members {
                if member.handle.WaitForSingleObject(Some(0))? != co::WAIT::OBJECT_0 {
                    exited = false;
                }
            }
            let empty = match self.child.as_ref() {
                Some(child) => child.active_process_count()? == Some(0),
                None => true,
            };
            if exited && empty {
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

fn capture_member(child: &ManagedChild, pid: u32, observed: &Value) -> Result<Option<Member>> {
    let handle = match HPROCESS::OpenProcess(
        co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
        false,
        pid,
    ) {
        Ok(handle) => handle,
        Err(co::ERROR::INVALID_PARAMETER) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let snapshot = identity::windows_snapshot(&handle, pid);
    let Some(identity) = identity::confirm_live_observation(snapshot, exited(&handle))? else {
        return Ok(None);
    };
    let in_job = child.member_pids()?.contains(&pid);
    // 成员可在重读 Job 时退出；仍活跃的句柄必须和原创建身份及 Job 成员表一致。
    if exited(&handle)? {
        return Ok(None);
    }
    check(
        identity == *observed && in_job,
        "Job 成员句柄创建身份或归属变化",
    )?;
    Ok(Some(Member { identity, handle }))
}

fn exited(handle: &HPROCESS) -> Result<bool> {
    Ok(handle.WaitForSingleObject(Some(0))? == co::WAIT::OBJECT_0)
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
    let observed = common::process_identity(pid).unwrap().unwrap();
    let captured = capture_member(&child, pid, &observed).unwrap().unwrap();
    assert_eq!(captured.identity, observed);

    let mut replaced = observed.clone();
    replaced["started"] = json!("1");
    assert!(capture_member(&child, pid, &replaced).is_err());
    let foreign = identity::current().unwrap();
    assert!(capture_member(&child, std::process::id(), &foreign).is_err());

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
    assert!(exited(&captured.handle).unwrap());
    assert!(capture_member(&child, pid, &observed).unwrap().is_none());
}
