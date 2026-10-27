//! 原生 API/Worker 独立监督交接；不更改浏览器命令的 ChildGroup 隔离策略。
//!
//! 角色引擎已有 owner/request/candidate/supervisor/result 和完整成员监督器。
//! 它的 start 控制器是短命进程，实际服务必须继续归原独立监督器所有。
//! 因此仅此已登记服务交接使用单独的、可回收的控制器句柄；其他依赖和
//! 浏览器命令继续使用 ChildGroup。接受交接前由 Rust 复核运行时、产品、
//! 树和全部存活身份，不把任意后台子进程当作成功服务。
use super::model::*;
use crate::{
    Result,
    recovery::{clone::native::resources::common, full_stack::worker::role::Role},
};
use serde_json::{Value, json};
use std::{
    process::{Child, Stdio},
    time::{Duration, Instant},
};

struct Controller(Child);
impl Drop for Controller {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
pub(super) fn control(
    context: &super::runtime::Runtime,
    role: Role,
    action: &str,
) -> Result<Value> {
    let output = &context.output;
    let (runtime, runtime_guard) = document(&output.join("runtime.json"))?;
    let scope = text(&runtime["scope_id"])?;
    check(
        !scope.is_empty(),
        &format!("{}交接缺少已登记scope", role.name()),
    )?;
    let mut command = context.controller(role, action)?;
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut controller = Controller(command.spawn()?);
    let stdout = controller
        .0
        .stdout
        .take()
        .ok_or_else(|| format!("{}交接缺少输出", role.name()))?;
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let _ = sender.send(super::process::capture(stdout, 1024 * 1024));
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = controller.0.try_wait()? {
            break status;
        }
        check(
            started.elapsed() < Duration::from_secs(300),
            &format!(
                "{}交接控制器超时；核对已登记owner与结果，不自动重放",
                role.name()
            ),
        )?;
        std::thread::sleep(Duration::from_millis(20));
    };
    let (bytes, overflow) = receiver
        .recv_timeout(Duration::from_secs(15))
        .map_err(|_| format!("{}交接输出未关闭", role.name()))??;
    reader
        .join()
        .map_err(|_| format!("{}交接输出线程失败", role.name()))?;
    check(
        status.success() && !overflow,
        &format!(
            "{}交接未确认；保留现有引擎的owner、结果和进程树",
            role.name()
        ),
    )?;
    let response: Value =
        serde_json::from_slice(&bytes).map_err(|_| format!("{}交接响应无效", role.name()))?;
    check(
        response["ok"] == true
            && response["result"]["outcome"] == "succeeded"
            && response["result"]["error"].is_null()
            && response["result"]["error_type"].is_null()
            && matches!(
                response["result"]["state"].as_str(),
                Some("running" | "stopped")
            )
            && response["result"]["operation"] == action
            && response["result"]["scope_id"] == scope,
        &format!("{}交接未返回运行状态", role.name()),
    )?;
    check(
        (action != "start" || response["result"]["state"] == "running")
            && (action != "stop" || response["result"]["state"] == "stopped"),
        "服务控制结果未达到请求目标",
    )?;
    if response["result"]["state"] == "running" {
        verify(output, scope, role)?;
    }
    unchanged(&runtime_guard)?;
    Ok(response["result"]["state"].clone())
}
fn verify(output: &std::path::Path, scope: &str, role: Role) -> Result<()> {
    let (tree, tree_guard) = document(&output.join(format!("{}-tree.json", role.name())))?;
    let (process, process_guard) = document(&output.join(format!("{}.json", role.name())))?;
    crate::recovery::clone::native::json::exact(
        &tree,
        &[
            "format_version",
            "kind",
            "runtime_directory",
            "role",
            "scope_id",
            "operation_id",
            "supervisor",
            "process",
            "monitor",
            "group_id",
        ],
        &format!("{}交接进程树", role.name()),
    )?;
    crate::recovery::clone::native::json::exact(
        &process,
        &["format_version", "role", "scope_id", "identity"],
        &format!("{}交接产品进程", role.name()),
    )?;
    check(
        tree["format_version"].as_u64() == Some(2)
            && tree["kind"] == "full-stack-process-tree"
            && tree["runtime_directory"] == json!(output)
            && tree["role"] == role.name()
            && tree["scope_id"] == scope
            && process["scope_id"] == scope
            && process["role"] == role.name()
            && process["identity"] == tree["process"],
        &format!("{}交接的运行时、产品与进程树不一致", role.name()),
    )?;
    for name in ["process", "supervisor", "monitor"] {
        let identity = common::validate_process_identity(&tree[name])?;
        let pid = identity["pid"]
            .as_u64()
            .and_then(|p| u32::try_from(p).ok())
            .ok_or_else(|| format!("{}交接PID无效", role.name()))?;
        check(
            common::process_identity(pid)? == Some(identity),
            &format!("{}交接时已登记身份退出或被复用", role.name()),
        )?;
    }
    check(
        regex::Regex::new(r"^[a-f0-9]{32}$")?.is_match(text(&tree["operation_id"])?)
            && tree["group_id"] == tree["supervisor"]["pid"]
            && tree["monitor"]["pid"] != tree["supervisor"]["pid"]
            && tree["monitor"]["pid"] != tree["process"]["pid"]
            && process["format_version"].as_u64() == Some(1),
        &format!("{}交接操作ID、进程组或监督身份无效", role.name()),
    )?;
    unchanged(&tree_guard)?;
    unchanged(&process_guard)
}
