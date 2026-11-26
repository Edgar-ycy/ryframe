//! 唯一 Worker 控制状态机：持锁复核来源，未知结果只登记和显式核对。
use super::{
    launch,
    operation::{self, Operation},
    process,
    role::Role,
    tree,
};
use crate::{Result, recovery::clone::native::json};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};

#[path = "controller/result.rs"]
mod result;
use result::finish;
#[path = "controller/observation.rs"]
mod observation;
pub(crate) use observation::observe;
use observation::verify_running;
#[path = "controller/api_incident.rs"]
mod api_incident;
pub(crate) use api_incident::observe_windows_api_incident;
#[path = "controller/bound.rs"]
mod bound;
pub(crate) use bound::{
    StopGeneration, StopTarget, control_bound, observe_bound, observe_bound_state,
    observe_worker_under_outer_lock, reconcile_completed_bound, reconcile_recorded_stop,
    require_api_tree, stop_bound_recorded,
};
#[path = "controller/archived_start.rs"]
mod archived_start;
#[cfg(test)]
pub(crate) use archived_start::tests::{STOP_ID_START, archive_stopped_fixture, stopped_runtime};
pub(crate) use archived_start::{ArchivedStop, archived_start_bound, archived_stop_bound};

#[derive(Debug)]
struct Pending(Role);
impl std::fmt::Display for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}后台启动仍在执行；稍后显式核对", self.0.label())
    }
}
impl std::error::Error for Pending {}

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/full_stack/worker/reconciliation_tests.rs"]
mod reconciliation_tests;

pub(super) fn guard(
    role: Role,
    directory: &Path,
    timeout: Duration,
) -> Result<super::guard::Guard> {
    let deadline = Instant::now() + timeout;
    loop {
        match super::guard::Guard::acquire(role, directory) {
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
pub(in crate::recovery) fn ready(url: &str) -> Result<bool> {
    super::probes::execute(&json!({"action":"ready","url":url,"timeout":0})).map(|v| v == true)
}
fn free(role: Role, runtime: &Value, timeout: f64) -> Result<()> {
    super::probes::execute(
        &json!({"action":"wait-free","url":runtime[role.ready()],"timeout":timeout}),
    )
    .map(|_| ())
}

pub(super) fn execute(role: Role, args: &Value) -> Result<Value> {
    json::exact(
        args,
        &["action", "backend", "directory", "timeout"],
        &format!("{}控制参数", role.label()),
    )?;
    let action = args["action"]
        .as_str()
        .ok_or_else(|| super::Rejection(format!("{}控制操作无效", role.label())))?;
    let timeout = args["timeout"]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.0 && *v <= 180.0)
        .ok_or_else(|| super::Rejection(format!("{}就绪超时必须在0到180秒之间", role.label())))?;
    let backend = operation::canonical(Path::new(args["backend"].as_str().ok_or("缺少后端目录")?))?;
    let directory =
        operation::canonical(Path::new(args["directory"].as_str().ok_or("缺少运行目录")?))?;
    control(role, action, &backend, &directory, timeout)
}
pub(crate) fn control(
    role: Role,
    action: &str,
    backend: &Path,
    directory: &Path,
    timeout: f64,
) -> Result<Value> {
    let started = Instant::now();
    if role == Role::Frontend {
        return Err("前端有限命令不能使用长期服务控制入口".into());
    }
    if std::env::var("APP_ENV").as_deref() != Ok("test") {
        return Err(super::Rejection(format!(
            "{}控制要求APP_ENV=test并选择隔离文件配置",
            role.label()
        ))
        .into());
    }
    control_verified(role, action, backend, directory, timeout, started, None)
}

struct BoundControl<'a> {
    environment: &'a super::environment::Environment,
    stop: Option<StopTarget<'a>>,
}

fn control_verified(
    role: Role,
    action: &str,
    backend: &Path,
    directory: &Path,
    timeout: f64,
    started: Instant,
    binding: Option<&BoundControl<'_>>,
) -> Result<Value> {
    if role == Role::Frontend {
        return Err("前端有限命令不能使用长期服务控制入口".into());
    }
    let (action, ci_api) = super::ci_api_action(role, action)?;
    if !timeout.is_finite() || !(0.0..=180.0).contains(&timeout) || timeout == 0.0 {
        return Err(super::Rejection(format!("{}操作或超时无效", role.label())).into());
    }
    let backend = operation::canonical(backend)?;
    let directory = operation::canonical(directory)?;
    let deadline = if action == "start" {
        started + super::CONTROL_TIMEOUT
    } else {
        Instant::now() + Duration::from_secs_f64(timeout + 10.0)
    };
    loop {
        if let Some(binding) = binding {
            binding.environment.check()?;
        }
        let result = once(
            role,
            (action, ci_api),
            &backend,
            &directory,
            timeout,
            deadline,
            binding,
        );
        if let Some(binding) = binding {
            binding.environment.check()?;
        }
        match result {
            Err(error) if error.is::<Pending>() && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50))
            }
            result => return result,
        }
    }
}

fn inputs(role: Role, policy: (&str, bool), root: &Path, dir: &Path) -> Result<(Value, String)> {
    let (action, ci_api) = policy;
    if ci_api && super::ci_api_evidence(dir)? != (action == "stop") {
        return Err("API 启动目录已有进程证据，或停止缺少完整进程树；保留现场".into());
    }
    let runtime = super::runtime::load(role, root, dir).map_err(|_| {
        super::Rejection(format!(
            "{}运行合同核验失败；核对冻结来源、配置与产物，保留现场且不启动或停止进程",
            role.label()
        ))
    })?;
    let source = operation::source(dir)?;
    if role != Role::Worker
        && dir.join(format!("{}.json", role.name())).exists()
        && !dir.join(format!("{}-tree.json", role.name())).exists()
    {
        return Err(super::Rejection(format!(
            "{}旧进程没有完整监督树；拒绝接管或补签，由原现场控制器收尾",
            role.label()
        ))
        .into());
    }
    if dir.join(role.lock()).exists() {
        if action == "start" {
            return Err(super::Rejection(format!(
                "已有{}控制操作，必须先使用status或reconcile核对",
                role.label()
            ))
            .into());
        }
        let empty = fs::read_dir(dir.join(role.lock()))?.next().is_none();
        if empty {
            if action != "reconcile" {
                return Err("发现旧空锁；必须显式reconcile核对".into());
            }
            legacy(role, dir, &runtime, &source)?;
        } else {
            reconcile(role, root, dir, &runtime, &source)?;
        }
    }
    if action == "start" {
        if process::registered(role, dir, &runtime)?.is_some() {
            return Err(
                super::Rejection(format!("已登记{}仍在运行，拒绝重复启动", role.label())).into(),
            );
        }
        if let Some(tree) = tree::read(dir, &runtime, role, false)? {
            if process::alive(&tree["supervisor"])? {
                return Err(
                    super::Rejection(format!("上一代{}监督器仍在运行", role.label())).into(),
                );
            }
            tree::closure(&tree, Duration::from_secs(5), true)?;
        }
        free(role, &runtime, 0.0)?;
    }
    if ci_api && action == "stop" {
        bound::require_api_tree(dir, &runtime)?;
    }
    Ok((runtime, source))
}
fn once(
    role: Role,
    (action, ci_api): (&str, bool),
    backend: &Path,
    directory: &Path,
    timeout: f64,
    deadline: Instant,
    binding: Option<&BoundControl<'_>>,
) -> Result<Value> {
    if action == "start" {
        super::check_result_deadline(role, deadline)?;
    }
    let guard = guard(role, directory, Duration::ZERO)?;
    if binding.is_some() || ci_api {
        bound::require_no_pending(role, directory)?;
    }
    let (runtime, source) = inputs(role, (action, ci_api), backend, directory)?;
    if action == "start" {
        super::check_result_deadline(role, deadline)?;
    }
    let stop = binding.and_then(|binding| binding.stop.as_ref());
    let claimed = if let Some(stop) = stop {
        stop.check(role, directory, &runtime)?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        (stop.record)(&id)?;
        guard.check()?;
        stop.check(role, directory, &runtime)?;
        Operation::claim_recorded_stop(role, backend, directory, &runtime, timeout, &id)
    } else {
        Operation::claim(role, action, backend, directory, &runtime, timeout)
    };
    let op = claimed.map_err(|_| {
        super::Rejection(format!(
            "{} {action} 的 owner/request 发布未确认；保留控制目录，不重放",
            role.label()
        ))
    })?;
    guard.check()?;
    if let Some(stop) = stop {
        op.check_source()?;
        stop.check(role, directory, &runtime)?;
        guard.check()?;
    }
    if action != "start" {
        let result = match simple(&op, &runtime, action) {
            Ok(result) => result,
            Err(_) => {
                let result = op.result(
                    "unknown",
                    Value::Null,
                    Some(&format!(
                        "{}状态或回收未确认；保留控制收据，核对创建身份、监听端口与完整成员证明",
                        role.label()
                    )),
                    false,
                )?;
                op.publish("result.json", &result, false)?;
                return Err(super::Rejection(format!(
                    "{}状态或回收未确认；保留控制锁，不自动重放",
                    role.label()
                ))
                .into());
            }
        };
        op.publish("result.json", &result, false)?;
        guard.check()?;
        let result = op.archive()?;
        if let Some(stop) = stop {
            let stopped_tree =
                tree::read(directory, &runtime, role, true)?.ok_or("原停止缺少进程树")?;
            stop.generation
                .prove_closed(role, directory, &runtime, &stopped_tree)?;
            (stop.publish)()?;
            guard.check()?;
        }
        return Ok(result);
    }
    start(backend, &runtime, &source, &op, guard, deadline)
}
pub(super) fn reserved_start(
    reservation: &super::start::Reservation,
    timeout: f64,
) -> Result<Value> {
    let deadline = Instant::now() + super::CONTROL_TIMEOUT;
    let guard = guard(reservation.role, &reservation.directory, Duration::ZERO)?;
    reservation.verify()?;
    let (runtime, source) = inputs(
        reservation.role,
        ("start", false),
        &reservation.backend,
        &reservation.directory,
    )?;
    super::check_result_deadline(reservation.role, deadline)?;
    let op = Operation::claim_reserved(reservation, &runtime, timeout)?;
    guard.check()?;
    reservation.verify()?;
    start(
        &reservation.backend,
        &runtime,
        &source,
        &op,
        guard,
        deadline,
    )
}

fn start(
    backend: &Path,
    runtime: &Value,
    source: &str,
    op: &Operation,
    guard: super::guard::Guard,
    deadline: Instant,
) -> Result<Value> {
    super::check_result_deadline(op.role, deadline)?;
    let args = launch::request(op.role, backend, op.directory(), op.id()?);
    let monitor = launch::detached("monitor", &args, op.directory());
    if monitor.is_err() {
        let result = op.result(
            "unknown",
            Value::Null,
            Some(&format!(
                "{}成员监督器启动未确认；核对收据，不自动重放",
                op.role.label()
            )),
            false,
        )?;
        op.publish("result.json", &result, false)?;
        return Err(super::Rejection(format!("{}成员监督器启动未确认", op.role.label())).into());
    }
    // 句柄关闭不终止已授权的服务；monitor 持有真正的整树所有权。
    let monitor = monitor?;
    let monitor_identity =
        crate::recovery::clone::native::resources::common::process_identity(monitor)?
            .ok_or("成员监督器登记前退出")?;
    if let Err(error) = authorize(op, &monitor_identity) {
        drop(guard);
        if error.is::<super::Rejection>() {
            return Err(error);
        }
        return Err(super::Rejection(format!(
            "{}启动授权未确认；保留候选、成员关闭证明和控制锁",
            op.role.label()
        ))
        .into());
    }
    guard.check()?;
    drop(guard);
    finish(backend, op.directory(), runtime, source, op, deadline)
}
pub(super) fn authorize(op: &Operation, monitor: &Value) -> Result<()> {
    let role = op.role;
    let preparation = super::preparation_timeout(role);
    let deadline = Instant::now() + preparation;
    loop {
        super::check_preparation(role, deadline)?;
        if op.path.join("candidate.json").exists() {
            let candidate = op.child("candidate.json")?;
            let ready = operation::read(&tree::path(op.directory(), role, op.id()?, "ready"))?;
            if ready["monitor"] != *monitor
                || ready["supervisor"] != candidate["identity"]
                || ready["operation_id"] != op.owner["operation_id"]
                || !process::alive(&candidate["identity"])?
                || !process::alive(monitor)?
            {
                return Err(
                    super::Rejection(format!("{}候选与实际监督进程不匹配", role.label())).into(),
                );
            }
            let mut supervisor = op.bound(&op.role.kind("supervisor"))?;
            supervisor["identity"] = candidate["identity"].clone();
            supervisor["candidate_sha256"] = json!(super::documents::hash(&candidate)?);
            supervisor["authorized_at_ns"] = json!(operation::now());
            super::check_preparation(role, deadline)?;
            op.publish("supervisor.json", &supervisor, false)?;
            super::check_preparation(role, deadline)?;
            return Ok(());
        }
        if !process::alive(monitor)? {
            let message = super::preparation::failure(op, monitor)
                .unwrap_or_else(|| format!("{}监督候选登记前成员监督器已退出", role.label()));
            return Err(super::Rejection(message).into());
        }
        super::check_preparation(role, deadline)?;
        thread::sleep(Duration::from_millis(20));
    }
}
fn simple(op: &Operation, runtime: &Value, action: &str) -> Result<Value> {
    let role = op.role;
    let directory = op.directory();
    let mut identity = process::registered(role, directory, runtime)?;
    let mut reconciled = false;
    if matches!(action, "stop" | "crash") {
        if let Some(tree) = tree::read(directory, runtime, role, identity.is_some())? {
            tree::stop(&tree, action == "crash")?;
        }
        free(role, runtime, 5.0)?;
        identity = None;
    } else if let Some(identity) = &identity {
        verify_running(role, directory, runtime, identity)?;
    } else {
        if let Some(tree) = tree::read(directory, runtime, role, false)? {
            if process::alive(&tree["supervisor"])? {
                return Err(
                    super::Rejection(format!("{}退出但监督器仍在运行", role.label())).into(),
                );
            }
            reconciled = tree::closure(&tree, Duration::from_secs(10), true)?["status"]
                == "external-termination";
        }
        free(role, runtime, 0.0)?;
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
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/full_stack/worker/bound_state_tests.rs"]
mod bound_state_tests;

fn archive_completed(op: &Operation) -> Result<Value> {
    let result = op.validate("result", operation::read(&op.path.join("result.json"))?)?;
    if result["operation"] == "start" && result["outcome"] == "failed" {
        for name in ["supervisor.json", "candidate.json"] {
            if op.path.join(name).exists() {
                let child = op.child(name)?;
                if process::alive(&child["identity"])? {
                    return Err(Pending(op.role).into());
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
fn reconcile(
    role: Role,
    backend: &Path,
    directory: &Path,
    runtime: &Value,
    source: &str,
) -> Result<Value> {
    let op = Operation::read(role, directory, source)?;
    op.validate("request", Value::Null)?;
    if op.request["backend_root"] != json!(backend) {
        return Err(super::Rejection(format!("{}操作后端漂移", role.label())).into());
    }
    if process::alive(&op.owner["identity"])? {
        return Err(
            super::Rejection(format!("{}控制器仍在运行，拒绝并发操作", role.label())).into(),
        );
    }
    if op.path.join("result.json").exists() {
        return archive_completed(&op);
    }
    for name in ["supervisor.json", "candidate.json"] {
        if op.path.join(name).exists() && process::alive(&op.child(name)?["identity"])? {
            return Err(Pending(op.role).into());
        }
    }
    let action = op.request["operation"].as_str().ok_or("操作无效")?;
    let result = if action == "start" {
        if let Some(identity) = process::registered(role, directory, runtime)? {
            if verify_running(role, directory, runtime, &identity).is_ok() {
                op.result("running", identity, None, true)?
            } else {
                if let Some(tree) = tree::read(directory, runtime, role, true)? {
                    tree::stop(&tree, true)?;
                }
                free(role, runtime, 5.0)?;
                op.result(
                    "stopped",
                    Value::Null,
                    Some(&format!("中断的{}控制未产生成功收据", role.label())),
                    true,
                )?
            }
        } else {
            if op.path.join("supervisor.json").exists() {
                tree::startup_cleanup(&op, &op.child("supervisor.json")?["identity"])?;
            }
            free(role, runtime, 0.0)?;
            op.result(
                "stopped",
                Value::Null,
                Some(&format!("中断的{}控制未产生成功收据", role.label())),
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
fn legacy(role: Role, directory: &Path, runtime: &Value, source: &str) -> Result<()> {
    if role != Role::Worker {
        return Err("该角色存在未知旧控制锁；保留现场，不接管旧启动".into());
    }
    let path = directory.join(role.lock());
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_dir()
        || crate::recovery::clone::native::digest::linked(&metadata)
        || fs::read_dir(&path)?.next().is_some()
    {
        return Err(super::Rejection(format!("旧{}锁不是空真实目录", role.label())).into());
    }
    let inode =
        crate::recovery::clone::native::resources::common::device_inode_for(&path, &metadata)?;
    let identity = process::registered(role, directory, runtime)?;
    if let Some(identity) = &identity {
        verify_running(role, directory, runtime, identity)?;
    } else {
        free(role, runtime, 0.0)?;
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
    let archive = directory.join(role.archive());
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
        return Err(super::Rejection(format!("旧{}锁在核对期间发生变化", role.label())).into());
    }
    operation::write_new(
        &archive.join(format!("legacy-{}.json", id.as_str().ok_or("ID无效")?)),
        &value,
    )?;
    fs::remove_dir(&path)?;
    Ok(())
}
