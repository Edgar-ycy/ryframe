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

pub(super) fn stop(backend: &Path, tree: &Value, crash: bool) -> Result<Value> {
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
            force(tree, crash)?;
        }
    }
    NativeRuntime::new(backend).complete_registered_tree(tree)?;
    check(
        common::binding(&root.join("rustfs-tree.json"))? == original,
        "停止后登记树变化",
    )?;
    Ok(json!(supervisor || process))
}

fn alive(expected: &Value) -> Result<bool> {
    common::validate_process_identity(expected)?;
    let pid = u32::try_from(expected["pid"].as_u64().ok_or("进程 PID 无效")?)?;
    match common::process_identity(pid)? {
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
fn force(tree: &Value, _crash: bool) -> Result<()> {
    use crate::recovery::clone::native::lifecycle::identity;
    use winsafe::{HPROCESS, co, prelude::kernel_Hprocess};
    check(
        alive(&tree["supervisor"])?,
        "Job 监督进程已退出但产品仍在，拒绝单进程回收",
    )?;
    let expected = &tree["supervisor"];
    let pid = u32::try_from(expected["pid"].as_u64().ok_or("监督进程 PID 无效")?)?;
    let handle = HPROCESS::OpenProcess(
        co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::TERMINATE | co::PROCESS::SYNCHRONIZE,
        false,
        pid,
    )?;
    if handle.WaitForSingleObject(Some(0))? != co::WAIT::OBJECT_0 {
        check(
            identity::windows_snapshot(&handle, pid)? == *expected,
            "停止句柄的创建身份或映像不同",
        )?;
        handle.TerminateProcess(137)?;
        check(
            handle.WaitForSingleObject(Some(5000))? == co::WAIT::OBJECT_0,
            "监督进程停止未确认；保留现场",
        )?;
    }
    check(
        wait(tree, Duration::from_secs(5))?,
        "Windows Job 树未在期限内退出；保留现场",
    )
}

#[cfg(unix)]
fn force(tree: &Value, crash: bool) -> Result<()> {
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
    for signal in [
        if crash {
            Signal::SIGKILL
        } else {
            Signal::SIGTERM
        },
        Signal::SIGKILL,
    ] {
        match killpg(Pid::from_raw(i32::try_from(group)?), signal) {
            Ok(()) | Err(Errno::ESRCH) => (),
            Err(error) => return Err(error.into()),
        }
        if wait(tree, Duration::from_secs(5))? {
            return Ok(());
        }
    }
    Err("原进程组未在期限内退出；保留现场".into())
}

#[cfg(not(any(windows, unix)))]
fn force(_tree: &Value, _crash: bool) -> Result<()> {
    Err("当前平台不能保持监督树停止语义，禁止裸 PID 控制".into())
}
