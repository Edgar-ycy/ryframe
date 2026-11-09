//! 只控制原收据的 RustFS 监督树；完整成员关闭证明不能由根 PID 消失替代。
use super::{
    Runtime,
    model::{check, text},
    readonly::NativeRuntime,
};
use crate::{
    Result,
    recovery::clone::native::{json as document, resources::common},
};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(target_os = "linux")]
#[path = "tree_control/linux.rs"]
mod linux;
#[cfg(any(windows, test))]
#[path = "tree_control/windows.rs"]
mod windows;
#[cfg(windows)]
pub(in crate::recovery) use windows::observe as observe_recovery_process;

pub(in crate::recovery) fn stop_recovery_process(expected: &Value) -> Result<bool> {
    #[cfg(any(windows, target_os = "linux"))]
    {
        common::validate_process_identity(expected)?;
        let pid = u32::try_from(expected["pid"].as_u64().ok_or("登记进程 PID 无效")?)?;
        #[cfg(target_os = "linux")]
        use linux as platform;
        #[cfg(windows)]
        use windows as platform;
        platform::stop_process(pid, expected)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = expected;
        Err("当前平台无法精确停止已登记进程，未发送信号".into())
    }
}

pub(in crate::recovery::fixture_control::services) fn stop(
    backend: &Path,
    tree: &Value,
    crash: bool,
) -> Result<Value> {
    let root = Path::new(text(tree, "runtime_directory")?);
    let original = common::binding(&root.join("rustfs-tree.json"))?;
    // 复用正式只读树解析，检查角色、scope、操作 ID 与所有成员身份。
    let registered = NativeRuntime::new(backend).call(
        "tree-read",
        json!({"output":root,"scope_id":text(tree, "scope_id")?}),
    )?;
    check(registered == *tree, "停止输入不是原登记的完整监督树")?;
    let supervisor = alive(&tree["supervisor"])?;
    let process = alive(&tree["process"])?;
    let mut terminated = supervisor || process;
    if supervisor || process {
        check(
            common::binding(&root.join("rustfs-tree.json"))? == original,
            "停止前登记树变化",
        )?;
        let operation = text(tree, "operation_id")?;
        let requested = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos())?;
        document::write_replace(
            &root.join(format!("rustfs-tree-{operation}-control.json")),
            &json!({"format_version":1,"kind":"full-stack-process-tree-control",
                "operation_id":operation,"mode":if crash {"crash"} else {"normal"},"requested_at_ns":requested}),
        )?;
        if crash || !supervisor || !wait(tree, Duration::from_secs(3))? {
            terminated = force(tree, crash, supervisor)?;
        }
    }
    NativeRuntime::new(backend).complete_registered_tree(tree)?;
    check(
        common::binding(&root.join("rustfs-tree.json"))? == original,
        "停止后登记树变化",
    )?;
    Ok(json!(terminated))
}

fn alive(expected: &Value) -> Result<bool> {
    common::validate_process_identity(expected)?;
    let pid = u32::try_from(expected["pid"].as_u64().ok_or("进程 PID 无效")?)?;
    #[cfg(windows)]
    let observed = windows::observe(pid)?;
    #[cfg(not(windows))]
    let observed = common::process_identity(pid)?;
    match observed {
        None => Ok(false),
        Some(actual) if actual == *expected => Ok(true),
        Some(_) => Err("登记 PID 已复用；不发送信号或推断整树关闭".into()),
    }
}

fn wait(tree: &Value, timeout: Duration) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        // 两个身份都必须观察，不能用短路隐藏第二个身份的未知状态。
        let supervisor = alive(&tree["supervisor"])?;
        let process = alive(&tree["process"])?;
        if !supervisor && !process {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(windows)]
fn force(tree: &Value, _crash: bool, initially_alive: bool) -> Result<bool> {
    use winsafe::{co, prelude::kernel_Hprocess};
    check(
        initially_alive,
        "Job 监督进程已退出但产品仍在，拒绝单进程回收",
    )?;
    let expected = &tree["supervisor"];
    let pid = u32::try_from(expected["pid"].as_u64().ok_or("监督进程 PID 无效")?)?;
    // 原监督器在正常等待后退出是合法竞争；不能重新观察 PID 后按未知拒绝。
    if let Some(handle) = windows::open(pid, true)? {
        windows::control_with(
            expected,
            || windows::snapshot(&handle, pid),
            || {
                handle.TerminateProcess(137)?;
                check(
                    handle.WaitForSingleObject(Some(5000))? == co::WAIT::OBJECT_0,
                    "监督进程停止未确认；保留现场",
                )
            },
        )?;
    }
    check(
        wait(tree, Duration::from_secs(5))?,
        "Windows Job 树未在期限内退出；保留现场",
    )?;
    Ok(true)
}

#[cfg(unix)]
fn force(tree: &Value, crash: bool, _initially_alive: bool) -> Result<bool> {
    use nix::{
        errno::Errno,
        sys::signal::{Signal, killpg},
        unistd::Pid,
    };
    let group = tree["group_id"]
        .as_i64()
        .filter(|v| *v > 1)
        .ok_or("进程组无效")?;
    for role in ["supervisor", "process"] {
        if alive(&tree[role])? {
            let pid = tree[role]["pid"].as_u64().ok_or("成员 PID 无效")?;
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
            let fields = stat
                .rsplit_once(')')
                .ok_or("进程 stat 无效")?
                .1
                .split_whitespace()
                .collect::<Vec<_>>();
            check(
                fields.get(2).and_then(|v| v.parse::<i64>().ok()) == Some(group)
                    && fields.get(3).and_then(|v| v.parse::<i64>().ok()) == Some(group),
                "进程已离开登记 session 或进程组",
            )?;
        }
    }
    let group = Pid::from_raw(i32::try_from(group)?);
    stop_group(
        crash,
        |kill| match killpg(
            group,
            if kill {
                Signal::SIGKILL
            } else {
                Signal::SIGTERM
            },
        ) {
            Ok(()) => Ok(true),
            Err(Errno::ESRCH) => Ok(false),
            Err(error) => Err(error.into()),
        },
        || wait(tree, Duration::from_secs(5)),
    )
}

#[cfg(any(unix, test))]
fn stop_group(
    crash: bool,
    mut send: impl FnMut(bool) -> Result<bool>,
    mut stopped: impl FnMut() -> Result<bool>,
) -> Result<bool> {
    // 首次 ESRCH 在完整 closure 后返回 false；升级 SIGKILL 时的错误不被吞掉。
    if !send(crash)? {
        return Ok(false);
    }
    if stopped()? {
        return Ok(true);
    }
    check(send(true)?, "升级终止时原进程组消失；保留原失败语义")?;
    check(stopped()?, "原进程组未在期限内退出；保留现场")?;
    Ok(true)
}

#[cfg(not(any(windows, unix)))]
fn force(_tree: &Value, _crash: bool, _initially_alive: bool) -> Result<bool> {
    Err("当前平台不能保持监督树停止语义，禁止裸 PID 控制".into())
}
