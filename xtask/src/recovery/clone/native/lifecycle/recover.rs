use std::path::Path;

use serde_json::{Value, json};

use super::{identity, lock, manifest, state};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{digest, json as json_io},
};

pub(crate) fn run(backend: &Path, run_dir: &Path, owner_binding: &Path) -> Result<Value> {
    validate_local_test_path(run_dir, backend, LocalTestPathKind::ExistingDirectory)
        .map_err(|error| format!("复制运行目录无效：{error}"))?;
    validate_local_test_path(owner_binding, backend, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("控制器绑定无效：{error}"))?;
    let guard = lock::ProcessGuard::acquire(run_dir)?;
    let descriptor_before = digest::file(owner_binding)?;
    let state_path = run_dir.join("state.json");
    let state_before = digest::file(&state_path)?;
    let owner = owner_from_binding(run_dir, owner_binding)?;
    identity::validate(&owner, run_dir)?;
    if owner.get("manifest_sha256").and_then(Value::as_str)
        != Some(
            digest::file(&run_dir.join("manifest.json"))?
                .sha256
                .as_str(),
        )
    {
        return Err("控制锁来源与固定验收目录不符".into());
    }
    let pid = owner["identity"]["pid"]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or("控制器 PID 无效")?;
    if identity::process(pid)?.is_some() {
        return Err("控制进程仍存在或 PID 已复用，拒绝回收".into());
    }
    let controller_number = controller_number(run_dir, owner_binding)?;
    let original_lock = lock::lock_snapshot(run_dir)?;
    if original_lock
        .as_ref()
        .is_some_and(|(_, _, observed)| *observed != owner)
    {
        return Err("现有控制锁属于其他控制器，不能接管".into());
    }
    unchanged(owner_binding, &descriptor_before)?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let intent_path = run_dir.join(format!("recovery-{id}.intent.json"));
    json_io::write_new(
        &intent_path,
        &json!({
            "status": "controller_recovery_started",
            "owner": manifest::binding(owner_binding, &descriptor_before)?,
            "state_before": manifest::binding(&state_path, &state_before)?,
        }),
    )?;
    unchanged(owner_binding, &descriptor_before)?;
    lock::check_recovery_snapshot(run_dir, &original_lock)?;
    if digest::file(&state_path)? != state_before {
        return Err("恢复前阶段状态变化，保留现场".into());
    }
    let recovered = state::interrupt_owned(run_dir, &owner, controller_number)?;
    unchanged(owner_binding, &descriptor_before)?;
    lock::recover_dead(run_dir, &owner, Some(&original_lock))?;
    guard.check()?;
    let result = json!({
        "status": "controller_recovered",
        "owner": owner,
        "remote_writes": 0,
        "copy_requires_reconciliation": true,
        "state_sha256": digest::plan_hash(&recovered)?,
    });
    json_io::write_new(&run_dir.join(format!("recovered-{id}.json")), &result)?;
    Ok(result)
}

fn owner_from_binding(run_dir: &Path, owner_binding: &Path) -> Result<Value> {
    if owner_binding == run_dir.join("run.lock/owner.json") {
        return json_io::read(owner_binding);
    }
    let state = state::load(run_dir, false)?;
    let attempt = state::running_attempt(&state)?.ok_or("持久控制器恢复必须对应唯一运行中阶段")?;
    let number = attempt["number"].as_u64().ok_or("运行中阶段序号无效")?;
    if owner_binding != state::controller_path(run_dir, number) {
        return Err("旧控制器不能恢复新的运行阶段".into());
    }
    let (_, owner) = state::controller_record(run_dir, number, attempt)?;
    Ok(owner)
}

fn controller_number(run_dir: &Path, owner_binding: &Path) -> Result<Option<u64>> {
    if owner_binding == run_dir.join("run.lock/owner.json") {
        return Ok(None);
    }
    let name = owner_binding
        .file_stem()
        .and_then(|value| value.to_str())
        .and_then(|value| value.strip_prefix("controller-"))
        .ok_or("持久控制器文件名无效")?;
    Ok(Some(name.parse::<u64>().map_err(|_| "持久控制器序号无效")?))
}

fn unchanged(path: &Path, expected: &digest::FileDigest) -> Result<()> {
    if digest::file(path)? != *expected {
        return Err("恢复期间控制器绑定发生变化，保留现场".into());
    }
    Ok(())
}
