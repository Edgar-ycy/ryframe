//! 前端有限命令复用同一授权、监督和成员证明，不把命令伪装成长期 Worker。
use super::{
    controller, launch,
    operation::{self, Operation},
    process,
    role::Role,
    runtime, tree,
};
use crate::{Result, process::ChildGroup};
use serde_json::{Value, json};
use std::{
    path::Path,
    process::Stdio,
    thread,
    time::{Duration, Instant},
};

pub(super) fn execute(args: &Value) -> Result<Value> {
    crate::recovery::clone::native::json::exact(
        args,
        &["backend", "directory", "request_binding"],
        "前端有限命令",
    )?;
    let backend = Path::new(args["backend"].as_str().ok_or("缺少后端目录")?).to_path_buf();
    let directory = Path::new(args["directory"].as_str().ok_or("缺少命令目录")?).to_path_buf();
    operation::canonical(&backend)?;
    operation::canonical(&directory)?;
    crate::recovery::fixture_runtime::verify_frontend_handoff(
        &directory,
        &args["request_binding"],
    )?;
    let mut runtime = runtime::load(Role::Frontend, &backend, &directory)?;
    crate::recovery::fixture_runtime::verify_frontend_handoff(
        &directory,
        &args["request_binding"],
    )?;
    runtime["request_binding"] = args["request_binding"].clone();
    let timeout = runtime["timeout_seconds"]
        .as_u64()
        .ok_or("前端命令缺少超时")?;
    let guard = controller::guard(Role::Frontend, &directory, Duration::ZERO)?;
    for name in [
        Role::Frontend.lock(),
        "frontend.json".into(),
        "frontend-tree.json".into(),
    ] {
        if directory.join(name).try_exists()? {
            return Err("前端命令已有意图或进程记录，保留现场，禁止重放".into());
        }
    }
    let op = Operation::claim(
        Role::Frontend,
        "run",
        &backend,
        &directory,
        &runtime,
        f64::from(u32::try_from(timeout)?),
    )?;
    run(&backend, &directory, &runtime, &op, guard, timeout)
}

fn run(
    backend: &Path,
    directory: &Path,
    runtime: &Value,
    op: &Operation,
    guard: super::guard::Guard,
    timeout: u64,
) -> Result<Value> {
    let args = launch::request(Role::Frontend, backend, directory, op.id()?);
    let mut command = launch::command("monitor", &args, directory)?;
    command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    let mut monitor = ChildGroup::new()?.spawn(command)?;
    let identity =
        crate::recovery::clone::native::resources::common::process_identity(monitor.id())?
            .ok_or("前端成员监督器登记前退出")?;
    controller::authorize(op, &identity)?;
    guard.check()?;
    drop(guard);
    let deadline = Instant::now() + Duration::from_secs(timeout + 15);
    loop {
        if let Some(status) = monitor.try_wait()? {
            if !status.success() {
                return Err("前端监督未确认成功，保留控制锁与全部进程收据，禁止重放".into());
            }
            break;
        }
        if Instant::now() >= deadline {
            if let Some(tree) = tree::read(directory, runtime, Role::Frontend, true)? {
                tree::stop(&tree, true)?;
            }
            return Err("前端命令超时；保留原命令、成员关闭证明和失败证据，禁止重放".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
    let guard = controller::guard(Role::Frontend, directory, Duration::from_secs(10))?;
    runtime::load(Role::Frontend, backend, directory)?;
    op.check()?;
    let tree = tree::read(directory, runtime, Role::Frontend, true)?.ok_or("前端命令缺少树")?;
    tree::closure(&tree, Duration::from_secs(5), false)?;
    if process::alive(&identity)? {
        return Err("前端成员监督器尚未退出".into());
    }
    let result = op.archive()?;
    guard.check()?;
    if result["outcome"] != "succeeded" || result["state"] != "stopped" {
        return Err("前端命令失败；完整收据已保留，不重放".into());
    }
    Ok(json!({"state":"completed","scope_id":runtime["scope_id"]}))
}
