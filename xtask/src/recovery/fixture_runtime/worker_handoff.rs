//! 原生 Worker 独立监督交接；不更改浏览器命令的 ChildGroup 隔离策略。
//!
//! Worker 引擎已有 owner/request/candidate/supervisor/result 和完整成员监督器。
//! 它的 start 控制器是短命进程，实际服务必须继续归原独立监督器所有。
//! 因此仅此已登记服务交接使用单独的、可回收的控制器句柄；其他依赖和
//! 浏览器命令继续使用 ChildGroup。接受交接前由 Rust 复核运行时、产品、
//! 树和全部存活身份，不把任意后台子进程当作成功服务。
use super::{
    dependency::{Dependencies, Python},
    model::*,
};
use crate::{Result, recovery::clone::native::resources::common};
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
pub(super) fn start(api: &mut Python, args: &Value) -> Result<Value> {
    api.call("verify", args.clone())?;
    let output = path(&args["output"])?;
    let (runtime, runtime_guard) = document(&output.join("runtime.json"))?;
    let scope = text(&runtime["scope_id"])?;
    check(!scope.is_empty(), "Worker交接缺少已登记scope")?;
    let mut command = crate::recovery::full_stack::worker::control_command(
        &path(&args["backend"])?,
        &output,
        "start",
        60.0,
    )?;
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut controller = Controller(command.spawn()?);
    let stdout = controller.0.stdout.take().ok_or("Worker交接缺少输出")?;
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
            "Worker交接控制器超时；核对已登记owner与结果，不自动重放",
        )?;
        std::thread::sleep(Duration::from_millis(20));
    };
    let (bytes, overflow) = receiver
        .recv_timeout(Duration::from_secs(15))
        .map_err(|_| "Worker交接输出未关闭")??;
    reader.join().map_err(|_| "Worker交接输出线程失败")?;
    check(
        status.success() && !overflow,
        "Worker交接未确认；保留现有引擎的owner、结果和进程树",
    )?;
    let response: Value = serde_json::from_slice(&bytes).map_err(|_| "Worker交接响应无效")?;
    check(
        response["ok"] == true
            && response["result"]["state"] == "running"
            && response["result"]["operation"] == "start"
            && response["result"]["scope_id"] == scope,
        "Worker交接未返回运行状态",
    )?;
    verify(&output, scope)?;
    unchanged(&runtime_guard)?;
    Ok(json!("running"))
}
fn verify(output: &std::path::Path, scope: &str) -> Result<()> {
    let (tree, tree_guard) = document(&output.join("worker-tree.json"))?;
    let (process, process_guard) = document(&output.join("worker.json"))?;
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
        "Worker交接进程树",
    )?;
    crate::recovery::clone::native::json::exact(
        &process,
        &["format_version", "role", "scope_id", "identity"],
        "Worker交接产品进程",
    )?;
    check(
        tree["format_version"].as_u64() == Some(2)
            && tree["kind"] == "full-stack-process-tree"
            && tree["runtime_directory"] == json!(output)
            && tree["role"] == "worker"
            && tree["scope_id"] == scope
            && process["scope_id"] == scope
            && process["role"] == "worker"
            && process["identity"] == tree["process"],
        "Worker交接的运行时、产品与进程树不一致",
    )?;
    for name in ["process", "supervisor", "monitor"] {
        let identity = common::validate_process_identity(&tree[name])?;
        let pid = identity["pid"]
            .as_u64()
            .and_then(|p| u32::try_from(p).ok())
            .ok_or("Worker交接PID无效")?;
        check(
            common::process_identity(pid)? == Some(identity),
            "Worker交接时已登记身份退出或被复用",
        )?;
    }
    check(
        regex::Regex::new(r"^[a-f0-9]{32}$")?.is_match(text(&tree["operation_id"])?)
            && tree["group_id"] == tree["supervisor"]["pid"]
            && tree["monitor"]["pid"] != tree["supervisor"]["pid"]
            && tree["monitor"]["pid"] != tree["process"]["pid"]
            && process["format_version"].as_u64() == Some(1),
        "Worker交接操作ID、进程组或监督身份无效",
    )?;
    unchanged(&tree_guard)?;
    unchanged(&process_guard)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::ChildGroup;
    #[test]
    fn handoff_requires_exact_live_product_supervisor_monitor_and_matching_scope() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();
        let api = Python::new(root).unwrap();
        let temp = super::super::tests::temporary();
        let mut children = vec![];
        for _ in 0..2 {
            let mut command = crate::process::child_command(&api.executable);
            command.args(["-c", "import time;time.sleep(30)"]);
            children.push(ChildGroup::new().unwrap().spawn(command).unwrap());
        }
        let supervisor = common::process_identity(children[0].id()).unwrap().unwrap();
        let monitor = common::process_identity(children[1].id()).unwrap().unwrap();
        let product = common::process_identity(std::process::id())
            .unwrap()
            .unwrap();
        let tree = json!({"format_version":2,"kind":"full-stack-process-tree","runtime_directory":temp.path(),"role":"worker","scope_id":"owned","operation_id":"a".repeat(32),"supervisor":supervisor,"monitor":monitor,"process":product,"group_id":supervisor["pid"]});
        write_new(&temp.path().join("worker-tree.json"), &tree).unwrap();
        write_new(
            &temp.path().join("worker.json"),
            &json!({"format_version":1,"role":"worker","scope_id":"owned","identity":product}),
        )
        .unwrap();
        verify(temp.path(), "owned").unwrap();
        assert!(verify(temp.path(), "other").is_err());
        drop(children.pop());
        assert!(verify(temp.path(), "owned").is_err());
    }
}
