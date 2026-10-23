//! 唯一 Worker 控制状态机：持锁复核来源，未知结果只登记和显式核对。
use super::{
    launch,
    operation::{self, LOCK, Operation},
    process, tree,
};
use crate::{Result, recovery::clone::native::json};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};

#[derive(Debug)]
struct Pending;
impl std::fmt::Display for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Worker后台启动仍在执行；稍后显式核对")
    }
}
impl std::error::Error for Pending {}

#[cfg(test)]
#[path = "reconciliation_tests.rs"]
mod reconciliation_tests;

pub(super) fn guard(directory: &Path, timeout: Duration) -> Result<super::guard::Guard> {
    let deadline = Instant::now() + timeout;
    loop {
        match super::guard::Guard::acquire(directory) {
            Ok(guard) => return Ok(guard),
            Err(error)
                if error.to_string() == "控制或锁恢复正在执行，拒绝并发操作"
                    && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(20))
            }
            Err(error) => return Err(error),
        }
    }
}
pub(super) fn ready(url: &str) -> Result<bool> {
    super::probes::execute(&json!({"action":"ready","url":url,"timeout":0})).map(|v| v == true)
}
fn free(runtime: &Value, timeout: f64) -> Result<()> {
    super::probes::execute(
        &json!({"action":"wait-free","url":runtime["worker_ready_url"],"timeout":timeout}),
    )
    .map(|_| ())
}

pub(super) fn execute(args: &Value) -> Result<Value> {
    json::exact(
        args,
        &["action", "backend", "directory", "timeout"],
        "Worker控制参数",
    )?;
    let action = args["action"]
        .as_str()
        .filter(|s| matches!(*s, "start" | "stop" | "crash" | "status" | "reconcile"))
        .ok_or("未知Worker控制操作")?;
    let timeout = args["timeout"]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.0 && *v <= 180.0)
        .ok_or("Worker就绪超时必须在0到180秒之间")?;
    let backend = operation::canonical(Path::new(args["backend"].as_str().ok_or("缺少后端目录")?))?;
    let directory =
        operation::canonical(Path::new(args["directory"].as_str().ok_or("缺少运行目录")?))?;
    control(action, &backend, &directory, timeout)
}
pub(crate) fn control(
    action: &str,
    backend: &Path,
    directory: &Path,
    timeout: f64,
) -> Result<Value> {
    if std::env::var("APP_ENV").as_deref() != Ok("test") {
        return Err("Worker控制要求APP_ENV=test并选择隔离文件配置".into());
    }
    if !matches!(action, "start" | "stop" | "crash" | "status" | "reconcile")
        || !timeout.is_finite()
        || !(0.0..=180.0).contains(&timeout)
        || timeout == 0.0
    {
        return Err("Worker操作或超时无效".into());
    }
    let deadline = Instant::now() + Duration::from_secs_f64(timeout + 10.0);
    loop {
        match once(action, backend, directory, timeout) {
            Err(error) if error.is::<Pending>() && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50))
            }
            result => return result,
        }
    }
}
fn once(action: &str, backend: &Path, directory: &Path, timeout: f64) -> Result<Value> {
    let guard = guard(directory, Duration::ZERO)?;
    let runtime = crate::recovery::fixture_control::verify_full_stack_runtime(backend, directory)?;
    let source = operation::source(directory)?;
    if directory.join(LOCK).exists() {
        if action == "start" {
            return Err("已有Worker控制操作，必须先使用status或reconcile核对".into());
        }
        let empty = fs::read_dir(directory.join(LOCK))?.next().is_none();
        if empty {
            if action != "reconcile" {
                return Err("发现旧空锁；必须显式reconcile核对".into());
            }
            legacy(directory, &runtime, &source)?;
        } else {
            reconcile(backend, directory, &runtime, &source)?;
        }
    }
    if action == "start" {
        if process::worker(directory, &runtime)?.is_some() {
            return Err("已登记Worker仍在运行，拒绝重复启动".into());
        }
        if let Some(tree) = tree::read(directory, &runtime, false)? {
            if process::alive(&tree["supervisor"])? {
                return Err("上一代Worker监督器仍在运行".into());
            }
            tree::closure(&tree, Duration::from_secs(5), true)?;
        }
        free(&runtime, 0.0)?;
    }
    let op = Operation::claim(action, backend, directory, &runtime, timeout)?;
    guard.check()?;
    if action != "start" {
        let result = match simple(&op, &runtime, action) {
            Ok(result) => result,
            Err(_) => {
                let result=op.result("unknown",Value::Null,Some("Worker状态或回收未确认；保留控制收据，核对创建身份、监听端口与完整成员证明"),false)?;
                op.publish("result.json", &result, false)?;
                return Err("Worker状态或回收未确认；保留控制锁，不自动重放".into());
            }
        };
        op.publish("result.json", &result, false)?;
        guard.check()?;
        return op.archive();
    }
    let args = launch::request(backend, directory, op.id()?);
    let monitor = launch::detached("monitor", &args, directory);
    if monitor.is_err() {
        let result = op.result(
            "unknown",
            Value::Null,
            Some("Worker成员监督器启动未确认；核对收据，不自动重放"),
            false,
        )?;
        op.publish("result.json", &result, false)?;
        return Err("Worker成员监督器启动未确认".into());
    }
    // 句柄关闭不终止已授权的服务；monitor 持有真正的整树所有权。
    let monitor = monitor?;
    let monitor_identity =
        crate::recovery::clone::native::resources::common::process_identity(monitor)?
            .ok_or("成员监督器登记前退出")?;
    let authorize = authorize(&op, &monitor_identity);
    if authorize.is_err() {
        drop(guard);
        return Err("Worker启动授权未确认；保留候选、成员关闭证明和控制锁".into());
    }
    guard.check()?;
    drop(guard);
    finish(backend, directory, &runtime, &source, &op, timeout)
}
fn authorize(op: &Operation, monitor: &Value) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if op.path.join("candidate.json").exists() {
            let candidate = op.child("candidate.json")?;
            let ready = operation::read(&tree::path(op.directory(), op.id()?, "ready"))?;
            if ready["monitor"] != *monitor
                || ready["supervisor"] != candidate["identity"]
                || ready["operation_id"] != op.owner["operation_id"]
                || !process::alive(&candidate["identity"])?
                || !process::alive(monitor)?
            {
                return Err("Worker候选与实际监督进程不匹配".into());
            }
            let mut supervisor = op.bound("full-stack-worker-control-supervisor")?;
            supervisor["identity"] = candidate["identity"].clone();
            supervisor["candidate_sha256"] = json!(super::documents::hash(&candidate)?);
            supervisor["authorized_at_ns"] = json!(operation::now());
            return op.publish("supervisor.json", &supervisor, false);
        }
        if !process::alive(monitor)? || Instant::now() >= deadline {
            return Err("Worker监督候选登记超时或退出".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
}
fn finish(
    backend: &Path,
    directory: &Path,
    runtime: &Value,
    source: &str,
    op: &Operation,
    timeout: f64,
) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs_f64(timeout + 10.0);
    loop {
        if op.path.join("result.json").exists() {
            let guard = guard(directory, Duration::from_secs(10))?;
            crate::recovery::fixture_control::verify_full_stack_runtime(backend, directory)?;
            if operation::source(directory)? != source {
                return Err("Worker等待期间来源发生变化".into());
            }
            let current = Operation::read(directory, source)?;
            if current.id()? != op.id()? {
                return Err("Worker等待期间操作发生变化".into());
            }
            let value = match archive_completed(&current) {
                Err(error) if error.is::<Pending>() => {
                    drop(guard);
                    thread::sleep(Duration::from_millis(20));
                    continue;
                }
                result => result?,
            };
            guard.check()?;
            if value["outcome"] != "succeeded" {
                return Err("Worker启动失败；已保留原始失败收据和完整成员关闭证明".into());
            }
            tree::running(directory, runtime, &value["identity"])?;
            process::listener(
                &value["identity"],
                runtime["worker_ready_url"].as_str().ok_or("端点无效")?,
            )?;
            return Ok(value);
        }
        if Instant::now() >= deadline {
            return Err("Worker后台启动结果未知；使用status或reconcile核对，不自动重放".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}
fn simple(op: &Operation, runtime: &Value, action: &str) -> Result<Value> {
    let directory = op.directory();
    let mut identity = process::worker(directory, runtime)?;
    let mut reconciled = false;
    if matches!(action, "stop" | "crash") {
        if let Some(tree) = tree::read(directory, runtime, identity.is_some())? {
            tree::stop(&tree, action == "crash")?;
        }
        free(runtime, 5.0)?;
        identity = None;
    } else if let Some(identity) = &identity {
        verify_running(directory, runtime, identity)?;
    } else {
        if let Some(tree) = tree::read(directory, runtime, false)? {
            if process::alive(&tree["supervisor"])? {
                return Err("Worker退出但监督器仍在运行".into());
            }
            reconciled = tree::closure(&tree, Duration::from_secs(10), true)?["status"]
                == "external-termination";
        }
        free(runtime, 0.0)?;
    }
    op.result(
        if identity.is_some() {
            "running"
        } else {
            "stopped"
        },
        identity.unwrap_or(Value::Null),
        None,
        reconciled,
    )
}
fn verify_running(directory: &Path, runtime: &Value, identity: &Value) -> Result<()> {
    let url = runtime["worker_ready_url"].as_str().ok_or("端点无效")?;
    if !ready(url)? {
        return Err("已登记Worker未通过就绪探针".into());
    }
    process::listener(identity, url)?;
    tree::running(directory, runtime, identity)?;
    Ok(())
}
fn archive_completed(op: &Operation) -> Result<Value> {
    let result = op.validate("result", operation::read(&op.path.join("result.json"))?)?;
    if result["operation"] == "start" && result["outcome"] == "failed" {
        for name in ["supervisor.json", "candidate.json"] {
            if op.path.join(name).exists() {
                let child = op.child(name)?;
                if process::alive(&child["identity"])? {
                    return Err(Pending.into());
                }
                if name == "supervisor.json" {
                    tree::startup_cleanup(op, &child["identity"])?;
                }
                break;
            }
        }
    }
    op.archive()
}
fn reconcile(backend: &Path, directory: &Path, runtime: &Value, source: &str) -> Result<Value> {
    let op = Operation::read(directory, source)?;
    op.validate("request", Value::Null)?;
    if op.request["backend_root"] != json!(backend) {
        return Err("Worker操作后端漂移".into());
    }
    if process::alive(&op.owner["identity"])? {
        return Err("Worker控制器仍在运行，拒绝并发操作".into());
    }
    if op.path.join("result.json").exists() {
        return archive_completed(&op);
    }
    for name in ["supervisor.json", "candidate.json"] {
        if op.path.join(name).exists() && process::alive(&op.child(name)?["identity"])? {
            return Err(Pending.into());
        }
    }
    let action = op.request["operation"].as_str().ok_or("操作无效")?;
    let result = if action == "start" {
        if let Some(identity) = process::worker(directory, runtime)? {
            if verify_running(directory, runtime, &identity).is_ok() {
                op.result("running", identity, None, true)?
            } else {
                if let Some(tree) = tree::read(directory, runtime, true)? {
                    tree::stop(&tree, true)?;
                }
                free(runtime, 5.0)?;
                op.result(
                    "stopped",
                    Value::Null,
                    Some("中断的Worker控制未产生成功收据"),
                    true,
                )?
            }
        } else {
            if op.path.join("supervisor.json").exists() {
                tree::startup_cleanup(&op, &op.child("supervisor.json")?["identity"])?;
            }
            free(runtime, 0.0)?;
            op.result(
                "stopped",
                Value::Null,
                Some("中断的Worker控制未产生成功收据"),
                true,
            )?
        }
    } else {
        let mut result = simple(&op, runtime, action)?;
        result["reconciled"] = json!(true);
        result
    };
    op.publish("result.json", &result, false)?;
    op.archive()
}
fn legacy(directory: &Path, runtime: &Value, source: &str) -> Result<()> {
    let path = directory.join(LOCK);
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_dir()
        || crate::recovery::clone::native::digest::linked(&metadata)
        || fs::read_dir(&path)?.next().is_some()
    {
        return Err("旧Worker锁不是空真实目录".into());
    }
    let inode =
        crate::recovery::clone::native::resources::common::device_inode_for(&path, &metadata)?;
    let identity = process::worker(directory, runtime)?;
    if let Some(identity) = &identity {
        verify_running(directory, runtime, identity)?;
    } else {
        free(runtime, 0.0)?;
    }
    let id = json!(uuid::Uuid::new_v4().simple().to_string());
    let mut value = operation::common(
        "full-stack-worker-legacy-lock-reconciliation",
        &id,
        directory,
        &json!(source),
    );
    value["reconciler_identity"] = process::current()?;
    value["state"] = json!(if identity.is_some() {
        "running"
    } else {
        "stopped"
    });
    value["worker_identity"] = identity.unwrap_or(Value::Null);
    value["completed_at_ns"] = json!(operation::now());
    let archive = directory.join(operation::ARCHIVE);
    if !archive.exists() {
        fs::create_dir(&archive)?;
    }
    operation::canonical(&archive)?;
    if crate::recovery::clone::native::resources::common::device_inode_for(
        &path,
        &fs::symlink_metadata(&path)?,
    )? != inode
        || fs::read_dir(&path)?.next().is_some()
    {
        return Err("旧Worker锁在核对期间发生变化".into());
    }
    operation::write_new(
        &archive.join(format!("legacy-{}.json", id.as_str().ok_or("ID无效")?)),
        &value,
    )?;
    fs::remove_dir(&path)?;
    Ok(())
}
