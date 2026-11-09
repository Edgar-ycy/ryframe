//! 只控制已登记的原监督树；正常退出失败后仍核对同一内核身份，不使用 taskkill 或裸 PID 接管。
use super::frame::Frame;
use crate::{
    Result,
    recovery::{
        clone::native::{json as document, resources::common},
        fixture_control::services::lifecycle::readonly::NativeRuntime,
        source::evidence::running::roles,
    },
};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub(super) fn stop(frame: &Frame<'_>, facts: &Value) -> Result<()> {
    let runtime = Path::new(document::text(&facts["output"], "源代次目录")?).join("runtime");
    let intent = document::read(
        &runtime
            .parent()
            .ok_or("源 runtime 缺少代次")?
            .join("intent.json"),
    )?;
    roles::directory(&runtime, &intent["operations"])?;
    let mut failures = Vec::new();
    for role in ["worker", "api"] {
        let result = (|| {
            frame.checkpoint()?;
            if runtime.join(format!("{role}-tree.json")).try_exists()? {
                let tree = roles::tree(&runtime, role, &facts["selected"]["scope_id"])?;
                if tree["operation_id"] != intent["operations"][role] {
                    return Err("源停止树与固定启动意图的 operation 不同".into());
                }
                stop_one(frame, &tree)?;
            }
            let url = if role == "api" {
                document::text(&facts["selected"]["api_url"], "源 API 地址")?
            } else {
                document::text(&facts["runtime"]["worker_ready_url"], "源 Worker 地址")?
            };
            crate::recovery::clone::native::target_verify::authority::require_closed(url)?;
            frame.checkpoint()
        })();
        if let Err(error) = result {
            failures.push(format!("{role}：{error}"));
        }
    }
    roles::directory(&runtime, &intent["operations"])?;
    if !failures.is_empty() {
        return Err(format!(
            "源双角色精确停止未完成：{}；保留全部树和关闭证据，禁止重放",
            failures.join("；")
        )
        .into());
    }
    Ok(())
}

fn stop_one(frame: &Frame<'_>, tree: &Value) -> Result<()> {
    let supervisor = alive(&tree["supervisor"])?;
    let process = alive(&tree["process"])?;
    if supervisor || process {
        let runtime = Path::new(document::text(&tree["runtime_directory"], "原进程树目录")?);
        let role = document::text(&tree["role"], "原进程树角色")?;
        if roles::tree(runtime, role, &tree["scope_id"])? != *tree {
            return Err("源进程树在发布停止请求前发生变化".into());
        }
        frame.checkpoint()?;
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let nanos = u64::try_from(nanos).map_err(|_| "源停止时间超出协议范围")?;
        let operation = document::text(&tree["operation_id"], "源树 operation")?;
        document::write_replace(
            &runtime.join(format!("{role}-tree-{operation}-control.json")),
            &json!({"format_version":1,"kind":"full-stack-process-tree-control",
                "operation_id":operation,"mode":"normal","requested_at_ns":nanos}),
        )?;
        if !(supervisor && wait_roots(tree, Duration::from_secs(3))?) {
            force(tree)?;
        }
    }
    // 只采用原 monitor 发布的完整成员证明；不以两个根 PID 消失代替所有成员关闭。
    NativeRuntime::new(frame.backend).complete_registered_tree(tree)?;
    frame.checkpoint()
}

fn alive(expected: &Value) -> Result<bool> {
    common::validate_process_identity(expected)?;
    let pid = u32::try_from(expected["pid"].as_u64().ok_or("源树 PID 无效")?)?;
    match common::process_identity(pid)? {
        None => Ok(false),
        Some(actual) if actual == *expected => Ok(true),
        Some(_) => Err("源进程 PID 已复用；拒绝操作其他内核代次".into()),
    }
}

fn wait_roots(tree: &Value, timeout: Duration) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        let observed =
            (|| -> Result<bool> { Ok(alive(&tree["supervisor"])? || alive(&tree["process"])?) })();
        match observed {
            Ok(false) => return Ok(true),
            Ok(true) => (),
            Err(error)
                if crate::recovery::clone::native::lifecycle::identity::is_access_denied(
                    error.as_ref(),
                ) && Instant::now() < deadline => {}
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(windows)]
fn force(tree: &Value) -> Result<()> {
    use crate::recovery::clone::native::lifecycle::identity;
    use winsafe::{HPROCESS, co, prelude::kernel_Hprocess};
    if !alive(&tree["supervisor"])? {
        return Err("Job 监督进程已退出但产品进程仍在，拒绝降级为单进程回收".into());
    }
    let expected = &tree["supervisor"];
    let pid = u32::try_from(expected["pid"].as_u64().ok_or("原监督进程 PID 无效")?)?;
    let handle = HPROCESS::OpenProcess(
        co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::TERMINATE | co::PROCESS::SYNCHRONIZE,
        false,
        pid,
    )
    .map_err(|error| {
        format!("源监督进程 {pid} 停止句柄打开失败：{error}；检查原进程权限，保留现场，不重试")
    })?;
    if handle.WaitForSingleObject(Some(0))? != co::WAIT::OBJECT_0 {
        if identity::windows_snapshot(&handle, pid)? != *expected {
            return Err("源监督进程句柄的创建身份或映像不同，拒绝终止".into());
        }
        handle.TerminateProcess(137)?;
        if handle.WaitForSingleObject(Some(5000))? != co::WAIT::OBJECT_0 {
            return Err("原监督进程未在5秒期限内退出；Job 树关闭结果未知，保留现场".into());
        }
    }
    if !wait_roots(tree, Duration::from_secs(5))? {
        return Err("Windows Job 未在期限内回收源监督进程和产品进程".into());
    }
    Ok(())
}

#[cfg(unix)]
fn force(tree: &Value) -> Result<()> {
    use nix::{
        errno::Errno,
        sys::signal::{Signal, killpg},
        unistd::Pid,
    };
    let group = tree["group_id"]
        .as_i64()
        .filter(|n| *n > 1)
        .ok_or("源进程组无效")?;
    for field in ["supervisor", "process"] {
        if alive(&tree[field])? {
            let pid = tree[field]["pid"].as_u64().ok_or("源成员 PID 无效")?;
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
            let (_, fields) = stat.rsplit_once(')').ok_or("源进程 stat 无效")?;
            let fields = fields.split_whitespace().collect::<Vec<_>>();
            if fields.get(2).and_then(|v| v.parse::<i64>().ok()) != Some(group)
                || fields.get(3).and_then(|v| v.parse::<i64>().ok()) != Some(group)
            {
                return Err("源进程已离开登记 session 或进程组，拒绝控制".into());
            }
        }
    }
    let group = Pid::from_raw(i32::try_from(group)?);
    for signal in [Signal::SIGTERM, Signal::SIGKILL] {
        match killpg(group, signal) {
            Ok(()) | Err(Errno::ESRCH) => (),
            Err(error) => {
                return Err(format!(
                    "源进程组发送 {signal:?} 失败：{error}；检查原组权限，保留现场"
                )
                .into());
            }
        }
        if wait_roots(tree, Duration::from_secs(5))? {
            return Ok(());
        }
    }
    Err("Unix 原进程组未在正常停止和强制停止期限内退出，保留现场".into())
}

#[cfg(not(any(windows, unix)))]
fn force(_tree: &Value) -> Result<()> {
    Err("当前平台不能保持原监督树停止语义，禁止退化为裸 PID 控制".into())
}
