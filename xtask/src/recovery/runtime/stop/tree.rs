//! 使用原恢复树协议及原监督器完成证明，不发布其它关闭收据。
use std::{
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::clone::native::{json as strict, resources::common},
};

pub(in crate::recovery::runtime) fn terminate(
    directory: &Path,
    role: &str,
    tree: &Value,
    checkpoint: impl Fn() -> Result<()>,
) -> Result<()> {
    let supervisor = root_alive(&tree["supervisor"])?;
    let process = root_alive(&tree["process"])?;
    if !supervisor && !process {
        return Ok(());
    }
    checkpoint()?;
    let operation = strict::text(&tree["operation_id"], "恢复进程树操作 ID")?;
    let requested = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos())?;
    let path = directory.join(format!("{role}-tree-{operation}-control.json"));
    verify_previous_control(&path, operation)?;
    strict::write_replace(
        &path,
        &json!({
            "format_version":1, "kind":"full-stack-process-tree-control",
            "operation_id":operation, "mode":"normal", "requested_at_ns":requested,
        }),
    )?;
    if supervisor && wait_roots(tree, Duration::from_secs(3))? {
        return Ok(());
    }
    checkpoint()?;
    force(tree, supervisor, checkpoint)
}

fn verify_previous_control(path: &Path, operation: &str) -> Result<()> {
    if !super::optional_file(path)? {
        return Ok(());
    }
    if std::fs::symlink_metadata(path)?.len() > 64 * 1024 {
        return Err("原进程树停止控制收据超过 64 KiB，保留现场".into());
    }
    let control = strict::read(path)?;
    strict::exact(
        &control,
        &[
            "format_version",
            "kind",
            "operation_id",
            "mode",
            "requested_at_ns",
        ],
        "原进程树停止控制收据",
    )?;
    if control["format_version"] != 1
        || control["kind"] != "full-stack-process-tree-control"
        || control["operation_id"] != operation
        || !matches!(control["mode"].as_str(), Some("normal" | "crash"))
        || control["requested_at_ns"].as_u64().is_none_or(|n| n == 0)
    {
        return Err("已有停止控制与原进程树操作不匹配，保留现场".into());
    }
    Ok(())
}

pub(in crate::recovery::runtime) fn require_exited(expected: &Value) -> Result<()> {
    if root_alive(expected)? {
        return Err("原成员仍在运行，保留待定状态".into());
    }
    Ok(())
}

fn root_alive(expected: &Value) -> Result<bool> {
    let pid = pid(expected)?;
    #[cfg(windows)]
    let actual = match open(pid, false)? {
        Some(handle) => observe(&handle, pid)?,
        None => None,
    };
    #[cfg(not(windows))]
    let actual = common::process_identity(pid)?;
    match actual {
        None => Ok(false),
        Some(actual) if actual == *expected => Ok(true),
        Some(_) => Err("恢复进程树 PID 已复用，拒绝控制其它内核代次".into()),
    }
}

fn pid(expected: &Value) -> Result<u32> {
    common::validate_process_identity(expected)?;
    Ok(u32::try_from(
        expected["pid"].as_u64().ok_or("恢复进程 PID 无效")?,
    )?)
}

fn wait_roots(tree: &Value, budget: Duration) -> Result<bool> {
    let deadline = Instant::now() + budget;
    loop {
        let supervisor = alive_during_exit(&tree["supervisor"])?;
        let process = alive_during_exit(&tree["process"])?;
        if !supervisor && !process {
            return Ok(true);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        std::thread::sleep(remaining.min(Duration::from_millis(20)));
    }
}

fn alive_during_exit(expected: &Value) -> Result<bool> {
    match root_alive(expected) {
        Ok(alive) => Ok(alive),
        #[cfg(windows)]
        Err(error) if access_denied(error.as_ref()) => Ok(true),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn access_denied(error: &(dyn std::error::Error + 'static)) -> bool {
    crate::recovery::clone::native::lifecycle::identity::is_access_denied(error)
        || error.downcast_ref::<winsafe::co::ERROR>() == Some(&winsafe::co::ERROR::ACCESS_DENIED)
}

#[cfg(windows)]
fn open(
    pid: u32,
    terminate: bool,
) -> Result<Option<winsafe::guard::CloseHandleGuard<winsafe::HPROCESS>>> {
    use winsafe::{HPROCESS, co, prelude::kernel_Hprocess};
    let mut access = co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE;
    if terminate {
        access |= co::PROCESS::TERMINATE;
    }
    match HPROCESS::OpenProcess(access, false, pid) {
        Ok(handle) => Ok(Some(handle)),
        Err(co::ERROR::INVALID_PARAMETER) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(windows)]
fn observe(handle: &winsafe::HPROCESS, pid: u32) -> Result<Option<Value>> {
    use crate::recovery::clone::native::lifecycle::identity;
    use winsafe::{co, prelude::kernel_Hprocess};
    let exited =
        || -> Result<bool> { Ok(handle.WaitForSingleObject(Some(0))? == co::WAIT::OBJECT_0) };
    if exited()? {
        return Ok(None);
    }
    identity::confirm_live_observation(identity::windows_snapshot(handle, pid), exited())
}

#[cfg(windows)]
fn force(
    tree: &Value,
    supervisor_was_alive: bool,
    checkpoint: impl Fn() -> Result<()>,
) -> Result<()> {
    use winsafe::prelude::kernel_Hprocess;
    if !supervisor_was_alive {
        return Err("原 Job 监督进程已退出但产品仍在，拒绝回收单个产品进程".into());
    }
    let expected = &tree["supervisor"];
    let pid = pid(expected)?;
    if let Some(handle) = open(pid, true)? {
        checkpoint()?;
        if let Some(actual) = observe(&handle, pid)? {
            if actual != *expected {
                return Err("停止句柄的内核创建身份或映像与原监督器不同".into());
            }
            handle.TerminateProcess(137)?;
        }
    }
    if !wait_roots(tree, Duration::from_secs(5))? {
        return Err("Windows Job 未在期限内回收原监督进程和产品进程".into());
    }
    Ok(())
}

#[cfg(unix)]
fn force(
    tree: &Value,
    _supervisor_was_alive: bool,
    checkpoint: impl Fn() -> Result<()>,
) -> Result<()> {
    use nix::{
        errno::Errno,
        sys::signal::{Signal, killpg},
        unistd::Pid,
    };
    let group = Pid::from_raw(i32::try_from(pid(&tree["supervisor"])?)?);
    checkpoint()?;
    if !verify_group(tree, group)? {
        return Ok(());
    }
    match killpg(group, Signal::SIGTERM) {
        Ok(()) => (),
        Err(Errno::ESRCH) => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    if wait_roots(tree, Duration::from_secs(5))? {
        return Ok(());
    }
    checkpoint()?;
    if !verify_group(tree, group)? {
        return Ok(());
    }
    killpg(group, Signal::SIGKILL)?;
    if !wait_roots(tree, Duration::from_secs(5))? {
        return Err("Unix 原进程组未在期限内退出，保留现场".into());
    }
    Ok(())
}

#[cfg(unix)]
fn verify_group(tree: &Value, group: nix::unistd::Pid) -> Result<bool> {
    use nix::unistd::{Pid, getpgid, getsid};
    let mut verified = false;
    for role in ["supervisor", "process"] {
        if root_alive(&tree[role])? {
            let pid = Pid::from_raw(i32::try_from(pid(&tree[role])?)?);
            if getpgid(Some(pid))? != group || getsid(Some(pid))? != group {
                return Err("原恢复进程已离开登记的 session 或进程组".into());
            }
            if !root_alive(&tree[role])? {
                return Err("校验原 session 时进程退出，保留现场".into());
            }
            verified = true;
        }
    }
    Ok(verified)
}

#[cfg(not(any(windows, unix)))]
fn force(
    _tree: &Value,
    _supervisor_was_alive: bool,
    _checkpoint: impl Fn() -> Result<()>,
) -> Result<()> {
    Err("当前平台无法安全回收原恢复进程树，保留现场".into())
}
