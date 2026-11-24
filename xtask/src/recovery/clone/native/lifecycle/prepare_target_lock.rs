//! 只回收首次私有运行文件尚未建立时被保留下来的目标空锁。
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fs, io::ErrorKind, path::Path};

use super::super::{
    copy_owner::Role,
    digest, json as document, post_process,
    resources::common,
    seed,
    target_verify::guard::{
        Inputs,
        prepare_lock::{self, Location, Observation},
    },
};
use super::{identity, lock, manifest, state};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

const KIND: &str = "devex-post-copy-prepare-target-lock";
const INTERRUPTED_KIND: &str = "devex-post-copy-prepare-target-lock-after-exit";

struct ObservationGuards {
    run: lock::ProcessGuard,
    target: lock::ProcessGuard,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    format_version: u8,
    kind: String,
    path: String,
    state_sha256: String,
    attempt: Value,
    controller: Value,
    owner: Value,
    context: PrepareContext,
    lock: Observation,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PrepareContext {
    manifest: Value,
    initialized: Value,
    registration: Value,
    attempt_directory: (u64, u64),
}

pub(crate) fn observe(
    backend: &Path,
    run: &Path,
    controller: &Path,
    output: &Path,
    after_exit: bool,
) -> Result<Value> {
    validate_local_test_path(run, backend, LocalTestPathKind::ExistingDirectory)?;
    validate_local_test_path(output, backend, LocalTestPathKind::OutputFile)?;
    post_process::files::absent(output)?;
    let root = Inputs::directory(output.parent().ok_or("观察收据缺少父目录")?)?;
    let guards = after_exit
        .then(|| acquire_observation_guards(backend, run))
        .transpose()?;
    let capture = || observe_prepare(backend, run, controller, output, guards.as_ref());
    let first = capture()?;
    let value = serde_json::to_value(&first)?;
    root.check()?;
    document::publish_new(output, &value, "准备目标空锁前置收据")?;
    // 如果阶段已越过前像边界，保留原观察，回收入口仍须重新核对现场。
    if first != capture()? {
        return Err("发布前置收据时原准备状态变化，保留现场".into());
    }
    root.check()?;
    if document::read(output)? != value {
        return Err("已发布的目标空锁前置收据变化".into());
    }
    Ok(value)
}

fn acquire_observation_guards(backend: &Path, run: &Path) -> Result<ObservationGuards> {
    if !cfg!(windows) {
        return Err("准备控制器退出后的空锁观察只支持 Windows".into());
    }
    let run_guard = lock::ProcessGuard::acquire_existing(run)?;
    let manifest = state::manifest_binding(run)?;
    let request = common::read_bound(backend, &manifest, "原运行清单")?;
    let initialized = initialized_path(backend, &request["initialized"])?;
    let target = initialized.parent().ok_or("已初始化目标缺少目录")?;
    let target_guard =
        lock::ProcessGuard::acquire_existing_named(target, Role::Target.guard_name())?;
    Ok(ObservationGuards {
        run: run_guard,
        target: target_guard,
    })
}

fn observe_prepare(
    backend: &Path,
    run: &Path,
    controller: &Path,
    output: &Path,
    held: Option<&ObservationGuards>,
) -> Result<Receipt> {
    let saved = state::load(run, true)?;
    let running = state::running_attempt(&saved)?.ok_or("当前没有运行中的准备阶段")?;
    let number = prepare_number(running)?;
    require_receipt_path(run, number, output)?;
    if state::attempts(&saved)?.last() != Some(running)
        || controller != state::controller_path(run, number)
    {
        return Err("必须精确绑定当前最后一个准备阶段控制器".into());
    }
    let (descriptor, owner) = state::controller_record(run, number, running)?;
    let outer = lock::lock_snapshot(run)?;
    if outer.as_ref().is_none_or(|(_, _, held)| *held != owner) {
        return Err("原准备控制器与运行锁归属不一致".into());
    }
    let context = context(backend, run, number, &saved)?;
    let live = held.is_none();
    require_process(run, &owner, &context.manifest, live)?;
    if let Some(guards) = held {
        guards.run.check()?;
    } else {
        lock::ProcessGuard::require_existing_named_held(run, "run-control.guard")?;
    }
    let initialized = initialized_path(backend, &context.initialized)?;
    let target = initialized.parent().ok_or("已初始化目标缺少目录")?;
    if let Some(guards) = held {
        guards
            .target
            .check_named(target, Role::Target.guard_name())?;
    } else {
        lock::ProcessGuard::require_existing_named_held(target, Role::Target.guard_name())?;
    }
    let empty = prepare_lock::observe(backend, target)?;
    if state::load(run, true)? != saved || lock::lock_snapshot(run)? != outer {
        return Err("读取前置收据时原准备或运行锁变化".into());
    }
    require_process(run, &owner, &context.manifest, live)?;
    if let Some(guards) = held {
        guards.run.check()?;
        guards
            .target
            .check_named(target, Role::Target.guard_name())?;
    }
    Ok(Receipt {
        format_version: 1,
        kind: if live { KIND } else { INTERRUPTED_KIND }.into(),
        path: manifest::path_text(output, "目标空锁前置收据")?,
        state_sha256: digest::plan_hash(&saved)?,
        attempt: running.clone(),
        controller: descriptor,
        owner,
        context,
        lock: empty,
    })
}

pub(crate) fn recover(backend: &Path, run: &Path, path: &Path) -> Result<Value> {
    recover_with_publisher(backend, run, path, |path, value| {
        document::publish_new(path, value, "准备目标空锁回收结果")
    })
}

fn recover_with_publisher(
    backend: &Path,
    run: &Path,
    path: &Path,
    publish: impl FnOnce(&Path, &Value) -> Result<()>,
) -> Result<Value> {
    if !cfg!(windows) {
        return Err("准备目标空锁回收只支持 Windows".into());
    }
    validate_local_test_path(run, backend, LocalTestPathKind::ExistingDirectory)?;
    let descriptor = common::binding(&common::common_file(backend, path, "原空锁观察")?)?;
    let receipt: Receipt =
        serde_json::from_value(common::read_bound(backend, &descriptor, "原空锁观察")?)?;
    if receipt.format_version != 1
        || !matches!(receipt.kind.as_str(), KIND | INTERRUPTED_KIND)
        || receipt.path != manifest::path_text(path, "原空锁观察")?
    {
        return Err("目标空锁观察格式或原路径无效".into());
    }
    require_receipt_path(run, prepare_number(&receipt.attempt)?, path)?;
    let intent_path = path.with_extension("recovering.json");
    let result_path = path.with_extension("recovered.json");
    let archive = path.with_extension("original-empty-lock");
    let root = Inputs::directory(path.parent().ok_or("目标空锁恢复缺少证据目录")?)?;
    let guard = lock::ProcessGuard::acquire_existing(run)?;
    let checked = || {
        root.check()?;
        guard.check()?;
        common::bound_file(backend, &descriptor, "原空锁观察")?;
        interrupted(backend, run, &receipt)
    };
    let state_binding = checked()?;
    let initialized = initialized_path(backend, &receipt.context.initialized)?;
    let target = initialized.parent().ok_or("已初始化目标缺少目录")?;
    let recovery = prepare_lock::Recovery::acquire(backend, target, &archive, receipt.lock)?;
    let intent = json!({
        "status": "prepare_target_lock_recovery_started",
        "observation": descriptor,
        "state": state_binding,
        "archive": manifest::path_text(&archive, "空锁归档")?,
    });
    let result = json!({
        "status": "prepare_target_lock_recovered",
        "intent": manifest::path_text(&intent_path, "空锁回收意图")?,
        "intent_sha256": digest::plan_hash(&intent)?,
        "attempt": receipt.attempt["number"],
        "remote_writes": 0,
    });
    post_process::files::absent(&result_path)?;
    let location = recovery.location()?;
    establish_intent(backend, &intent_path, &intent, location)?;
    if checked()? != state_binding {
        return Err("回收期间原中断状态变化".into());
    }
    let intent_binding = common::binding(&intent_path)?;
    if location == Location::Target {
        recovery.archive()?;
    }
    require_recovery_image(
        backend,
        &recovery,
        &intent_binding,
        checked()?,
        &state_binding,
    )?;
    publish(&result_path, &result)?;
    require_recovery_image(
        backend,
        &recovery,
        &intent_binding,
        checked()?,
        &state_binding,
    )?;
    let published = common::binding(&result_path)?;
    if common::read_bound(backend, &published, "目标空锁回收结果")? != result {
        return Err("刚发布的空锁回收结果变化".into());
    }
    Ok(result)
}

fn establish_intent(
    backend: &Path,
    path: &Path,
    expected: &Value,
    location: Location,
) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound && location == Location::Target => {
            document::publish_new(path, expected, "准备目标空锁回收意图")?;
        }
        Err(error) => return Err(error.into()),
        Ok(_) => (),
    }
    let binding = common::binding(path)?;
    if common::read_bound(backend, &binding, "目标空锁回收意图")? != *expected {
        return Err("目标空锁现状与原回收意图不符，保留现场".into());
    }
    Ok(())
}

fn require_recovery_image(
    backend: &Path,
    recovery: &prepare_lock::Recovery<'_>,
    intent: &Value,
    current_state: Value,
    expected_state: &Value,
) -> Result<()> {
    common::bound_file(backend, intent, "目标空锁回收意图")?;
    if &current_state != expected_state || recovery.location()? != Location::Archived {
        return Err("空锁归档后状态或锁身份变化，保留现场".into());
    }
    Ok(())
}

fn interrupted(backend: &Path, run: &Path, receipt: &Receipt) -> Result<Value> {
    let before = common::binding(&run.join("state.json"))?;
    let saved = state::load(run, true)?;
    let number = interrupted_number(&saved, receipt)?;
    let (controller, owner) = state::controller_record(run, number, &receipt.attempt)?;
    if controller != receipt.controller
        || owner != receipt.owner
        || lock::lock_snapshot(run)?.is_some()
        || context(backend, run, number, &saved)? != receipt.context
    {
        return Err("原准备控制器、锁或已登记上下文变化".into());
    }
    require_process(run, &owner, &receipt.context.manifest, false)?;
    common::bound_file(backend, &before, "中断状态")?;
    Ok(before)
}

fn interrupted_number(saved: &Value, receipt: &Receipt) -> Result<u64> {
    let number = prepare_number(&receipt.attempt)?;
    if state::running_attempt(saved)?.is_some() {
        return Err("先使用正式入口收尾原准备控制器".into());
    }
    let attempt = state::attempts(saved)?.last().ok_or("缺少原准备阶段")?;
    let mut expected = receipt.attempt.clone();
    expected["status"] = json!("failed");
    expected["error_type"] = json!("ControllerInterrupted");
    expected["finished_at"] = attempt["finished_at"].clone();
    if attempt != &expected || !attempt["finished_at"].is_string() {
        return Err("最新阶段不是同一准备的正式中断后像".into());
    }
    let mut running = saved.clone();
    let last = running["attempts"]
        .as_array_mut()
        .and_then(|rows| rows.last_mut())
        .ok_or("原阶段状态无法还原")?;
    *last = receipt.attempt.clone();
    if digest::plan_hash(&running)? != receipt.state_sha256 {
        return Err("原准备之前的同一运行历史变化".into());
    }
    Ok(number)
}

fn prepare_number(attempt: &Value) -> Result<u64> {
    if attempt["stage"] != "post-copy"
        || attempt["mode"] != "prepare"
        || attempt["status"] != "running"
        || !attempt["result"].is_null()
        || !attempt["error_type"].is_null()
        || !attempt["finished_at"].is_null()
    {
        return Err("前置收据必须来自尚未写出结果的准备阶段".into());
    }
    attempt["number"]
        .as_u64()
        .filter(|number| *number > 0)
        .ok_or_else(|| "准备阶段序号无效".into())
}

fn require_receipt_path(run: &Path, number: u64, path: &Path) -> Result<()> {
    if path != run.join(format!("prepare-target-lock-{number:04}.json")) {
        return Err("目标空锁观察必须使用原运行根下的固定阶段收据名".into());
    }
    Ok(())
}

fn require_process(run: &Path, owner: &Value, manifest: &Value, live: bool) -> Result<()> {
    identity::validate(owner, run)?;
    let pid = u32::try_from(owner["identity"]["pid"].as_u64().ok_or("控制器 PID 无效")?)?;
    let current = identity::process(pid)?;
    if owner["manifest_sha256"] != manifest["sha256"]
        || (live && current.as_ref() != Some(&owner["identity"]))
        || (!live && current.is_some())
    {
        return Err("原控制器的内核身份或退出状态无法精确证明".into());
    }
    Ok(())
}

fn context(backend: &Path, run: &Path, number: u64, saved: &Value) -> Result<PrepareContext> {
    let attempts = state::attempts(saved)?;
    let register = attempts
        .iter()
        .find(|row| {
            row["stage"] == "post-copy" && row["mode"] == "register" && row["status"] == "passed"
        })
        .ok_or("缺少原复制后登记结果")?;
    let active = seed::history::post(backend, run, None, Some(number), false)?;
    let active_number = attempts
        .iter()
        .rfind(|row| {
            row["stage"] == "post-copy"
                && matches!(row["mode"].as_str(), Some("register" | "amend"))
        })
        .and_then(|row| row["number"].as_u64())
        .ok_or("活动登记序号无效")?;
    if attempts.iter().any(|row| {
        row["stage"] == "post-copy"
            && row["mode"] == "prepare"
            && row["status"] == "passed"
            && row["number"].as_u64().is_some_and(|n| n > active_number)
    }) {
        return Err("当前登记已有正式准备结果，不能回收为未写入前像".into());
    }
    let manifest = state::manifest_binding(run)?;
    let request = common::read_bound(backend, &manifest, "原运行清单")?;
    let initialized = request["initialized"].clone();
    let initial = common::read_bound(backend, &initialized, "已初始化目标")?;
    if initial["status"] != "fresh_target_initialized" {
        return Err("清单未绑定已发布的目标初始化结果".into());
    }
    let attempt_directory = require_unwritten(backend, run, number, &active.runtime, &initial)?;
    Ok(PrepareContext {
        manifest,
        initialized,
        registration: json!({"register": register["result"], "active": active.descriptor}),
        attempt_directory,
    })
}

fn initialized_path(backend: &Path, initialized: &Value) -> Result<std::path::PathBuf> {
    let path = common::bound_file(backend, initialized, "已初始化目标")?;
    if path.file_name().and_then(|name| name.to_str()) != Some("initialized.json") {
        return Err("清单的已初始化结果路径无效".into());
    }
    Ok(path)
}

fn require_unwritten(
    backend: &Path,
    run: &Path,
    number: u64,
    runtime: &Path,
    initial: &Value,
) -> Result<(u64, u64)> {
    let name = runtime
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("准备运行目录名无效")?;
    post_process::files::absent(runtime)?;
    post_process::files::absent(&runtime.with_file_name(format!("{name}-configuration")))?;
    post_process::files::absent(&state::result_path(run, number))?;
    let directory = run.join("post-copy").join(format!("attempt-{number:04}"));
    validate_local_test_path(&directory, backend, LocalTestPathKind::ExistingDirectory)?;
    let identity = common::device_inode_for(&directory, &fs::symlink_metadata(&directory)?)?;
    for name in [
        "session-launch.json",
        "session-process.json",
        "session.stderr.log",
        "session.stdout.log",
    ] {
        post_process::files::absent(&directory.join(name))?;
    }
    let selected = &initial["generation"]["selected"];
    let product = Path::new(document::text(&selected["runtime_dir"], "原产品运行目录")?);
    validate_local_test_path(product, backend, LocalTestPathKind::StateDirectory)?;
    post_process::files::absent(product)?;
    common::require_closed_runtime_ports(
        document::text(&selected["api_url"], "原 API 地址")?,
        document::text(&selected["worker_ready_url"], "原 Worker 地址")?,
    )?;
    Ok(identity)
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/lifecycle/prepare_target_lock.rs"]
mod tests;
