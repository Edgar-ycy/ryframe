//! 调用方已持有进程互斥；只恢复绑定的死亡控制器，不接管未知锁。
use super::model::{binding, check, read};
use crate::{
    Result,
    recovery::clone::native::{
        digest, json as document,
        lifecycle::{identity, lock, state},
        resources::common,
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn run(backend: &Path, run: &Path, descriptor: &Value) -> Result<Value> {
    let path = common::bound_file(backend, descriptor, "待恢复控制器")?;
    let before = binding(&run.join("state.json"))?;
    let mut saved = state::load(run, true)?;
    let previous = saved.clone();
    let (owner, number) = owner(run, &path, descriptor, &saved)?;
    identity::validate(&owner, run)?;
    check(
        owner["manifest_sha256"] == saved["manifest"]["sha256"],
        "控制锁来源与固定验收目录不符",
    )?;
    let pid = u32::try_from(owner["identity"]["pid"].as_u64().ok_or("控制器 PID 无效")?)?;
    check(
        common::process_identity(pid)?.is_none(),
        "控制进程仍存在或 PID 已复用，拒绝回收",
    )?;
    let original_lock = lock_snapshot(run)?;
    if let Some((_, _, observed)) = &original_lock {
        check(*observed == owner, "现有控制锁属于其他控制器，不能接管")?;
    }
    interrupt(run, &mut saved, &owner, number)?;
    let after_sha = digest::plan_hash(&saved)?;
    let result = json!({"status":"controller_recovered","owner":owner,"remote_writes":0,
        "copy_requires_reconciliation":true,"state_sha256":after_sha});
    let id = uuid::Uuid::new_v4().simple().to_string();
    document::write_new(
        &run.join(format!("recovery-{id}.intent.json")),
        &json!({
        "status":"controller_recovery_started","owner":descriptor,
        "state_before":before,"state_after_sha256":after_sha}),
    )?;
    unchanged(run, &path, descriptor, &original_lock)?;
    check(
        binding(&run.join("state.json"))? == before,
        "恢复前阶段状态变化",
    )?;
    super::ledger::write(run, &previous, &saved)?;
    unchanged(run, &path, descriptor, &original_lock)?;
    lock::recover_dead(run, &owner)?;
    document::write_new(&run.join(format!("recovered-{id}.json")), &result)?;
    Ok(result)
}

type LockSnapshot = Option<((u64, u64), Value, Value)>;

fn lock_snapshot(run: &Path) -> Result<LockSnapshot> {
    let observed = lock::owner_observation(run)?;
    observed
        .map(|(descriptor, owner)| {
            let path = run.join("run.lock");
            Ok((
                common::device_inode_for(&path, &std::fs::symlink_metadata(&path)?)?,
                descriptor,
                owner,
            ))
        })
        .transpose()
}

fn unchanged(run: &Path, path: &Path, descriptor: &Value, original: &LockSnapshot) -> Result<()> {
    check(
        binding(path)? == *descriptor && lock_snapshot(run)? == *original,
        "恢复期间控制器绑定或控制锁身份变化；保留现场",
    )
}

fn owner(
    run: &Path,
    path: &Path,
    descriptor: &Value,
    saved: &Value,
) -> Result<(Value, Option<u64>)> {
    if path == run.join("run.lock/owner.json") {
        return Ok((read(path)?, None));
    }
    let attempt = state::running_attempt(saved)?.ok_or("持久控制器恢复缺少唯一运行中阶段")?;
    let number = attempt["number"].as_u64().ok_or("运行中阶段序号无效")?;
    check(
        path == state::controller_path(run, number),
        "旧控制器不能恢复新的运行阶段",
    )?;
    let (current, owner) = state::controller_record(run, number, attempt)?;
    check(current == *descriptor, "持久控制器绑定变化")?;
    Ok((owner, Some(number)))
}

fn interrupt(run: &Path, saved: &mut Value, owner: &Value, selected: Option<u64>) -> Result<()> {
    for attempt in saved["attempts"].as_array_mut().ok_or("阶段记录无效")? {
        if attempt["status"] != "running" {
            continue;
        }
        let number = attempt["number"].as_u64().ok_or("运行中阶段序号无效")?;
        if state::controller_path(run, number).try_exists()?
            && state::controller_record(run, number, attempt)?.1 != *owner
        {
            continue;
        }
        if selected.is_none_or(|expected| expected == number) {
            attempt["status"] = json!("failed");
            attempt["finished_at"] =
                json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false));
            attempt["error_type"] = json!("ControllerInterrupted");
        }
    }
    Ok(())
}
