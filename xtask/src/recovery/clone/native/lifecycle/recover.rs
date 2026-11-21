use std::{
    fs,
    path::{Path, PathBuf},
};

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
    let saved = state::load(run_dir, false)?;
    let running = state::running_attempt(&saved)?;
    let original_lock = lock::lock_snapshot(run_dir)?;
    let retry = running.is_none() && original_lock.is_some();
    let attempt = if retry {
        Some(interrupted_attempt(&saved)?)
    } else {
        running
    };
    let (owner, number) = owner_from_binding(run_dir, owner_binding, attempt, retry)?;
    verify_dead_owner(run_dir, &owner)?;
    if original_lock
        .as_ref()
        .is_some_and(|(_, _, observed)| *observed != owner)
    {
        return Err("现有控制锁属于其他控制器，不能接管".into());
    }
    unchanged(owner_binding, &descriptor_before, "控制器绑定")?;
    let descriptor = manifest::binding(owner_binding, &descriptor_before)?;
    let lock_binding = serde_json::to_value(&original_lock)?;
    let (id, after_sha256) = if retry {
        let after_sha256 = digest::plan_hash(&saved)?;
        let (id, path, expected) =
            pending_intent(run_dir, &descriptor, &lock_binding, &after_sha256)?;
        unchanged(&path, &expected, "原控制恢复意图")?;
        unchanged(&state_path, &state_before, "阶段状态")?;
        (id, after_sha256)
    } else {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let path = run_dir.join(format!("recovery-{id}.intent.json"));
        let recovered = state::interrupt_owned(run_dir, &owner, number, |updated| {
            json_io::write_new(
                &path,
                &json!({
                    "status": "controller_recovery_started",
                    "owner": descriptor,
                    "state_before": manifest::binding(&state_path, &state_before)?,
                    "state_after_sha256": digest::plan_hash(updated)?,
                    "lock": lock_binding,
                }),
            )?;
            unchanged(owner_binding, &descriptor_before, "控制器绑定")?;
            lock::check_recovery_snapshot(run_dir, &original_lock)?;
            unchanged(&state_path, &state_before, "阶段状态")
        })?;
        (id, digest::plan_hash(&recovered)?)
    };
    let result_path = run_dir.join(format!("recovered-{id}.json"));
    if receipt_exists(&result_path)? {
        return Err("控制恢复结果已存在，保留现场".into());
    }
    unchanged(owner_binding, &descriptor_before, "控制器绑定")?;
    guard.check()?;
    lock::recover_dead(run_dir, &owner, Some(&original_lock))?;
    guard.check()?;
    let result = json!({
        "status": "controller_recovered",
        "owner": owner,
        "remote_writes": 0,
        "copy_requires_reconciliation": true,
        "state_sha256": after_sha256,
    });
    json_io::write_new(&result_path, &result)?;
    Ok(result)
}

fn owner_from_binding(
    run_dir: &Path,
    owner_binding: &Path,
    attempt: Option<&Value>,
    retry: bool,
) -> Result<(Value, Option<u64>)> {
    if owner_binding == run_dir.join("run.lock/owner.json") {
        return Ok((json_io::read(owner_binding)?, None));
    }
    let attempt = attempt.ok_or("持久控制器恢复必须对应唯一运行中阶段")?;
    let number = attempt["number"].as_u64().ok_or("控制器阶段序号无效")?;
    if owner_binding != state::controller_path(run_dir, number) {
        return Err("旧控制器不能恢复新的运行阶段".into());
    }
    let mut running;
    let attempt = if retry {
        running = attempt.clone();
        running["status"] = json!("running");
        running["finished_at"] = Value::Null;
        running["error_type"] = Value::Null;
        &running
    } else {
        attempt
    };
    let (_, owner) = state::controller_record(run_dir, number, attempt)?;
    Ok((owner, Some(number)))
}

fn verify_dead_owner(run_dir: &Path, owner: &Value) -> Result<()> {
    identity::validate(owner, run_dir)?;
    if owner["manifest_sha256"] != digest::file(&run_dir.join("manifest.json"))?.sha256 {
        return Err("控制锁来源与固定验收目录不符".into());
    }
    let pid = owner["identity"]["pid"]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or("控制器 PID 无效")?;
    if identity::process(pid)?.is_some() {
        return Err("控制进程仍存在或 PID 已复用，拒绝回收".into());
    }
    Ok(())
}

fn interrupted_attempt(saved: &Value) -> Result<&Value> {
    state::attempts(saved)?
        .last()
        .filter(|attempt| {
            attempt["status"] == "failed"
                && attempt["error_type"] == "ControllerInterrupted"
                && attempt["finished_at"].is_string()
                && attempt["result"].is_null()
        })
        .ok_or_else(|| "控制锁接续必须对应最新已中断阶段".into())
}

fn pending_intent(
    run_dir: &Path,
    descriptor: &Value,
    lock_binding: &Value,
    after_sha256: &str,
) -> Result<(String, PathBuf, digest::FileDigest)> {
    let state_path = manifest::path_text(&run_dir.join("state.json"), "恢复前阶段状态路径")?;
    let mut pending = None;
    for entry in fs::read_dir(run_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|name| name.strip_prefix("recovery-")?.strip_suffix(".intent.json"))
        else {
            continue;
        };
        if !lower_hex(id, 32) {
            return Err("控制恢复意图文件名无效，保留现场".into());
        }
        let path = entry.path();
        if entry.metadata()?.len() > json_io::MAX_JSON_BYTES {
            return Err("控制恢复意图超过 16 MiB，保留现场".into());
        }
        let before = digest::file(&path)?;
        let intent = json_io::read(&path)?;
        unchanged(&path, &before, "原控制恢复意图")?;
        if !valid_intent(&intent, &state_path)? || intent["state_after_sha256"] != after_sha256 {
            continue;
        }
        let result_path = run_dir.join(format!("recovered-{id}.json"));
        if receipt_exists(&result_path)? {
            verify_receipt(run_dir, &result_path, &intent)?;
            return Err("控制恢复结果已存在，保留现场".into());
        }
        if pending.is_some() {
            return Err("存在多份未完成的控制恢复意图，保留现场".into());
        }
        if intent["owner"] != *descriptor || intent["lock"] != *lock_binding {
            return Err("控制恢复意图与当前控制器或控制锁不一致，保留现场".into());
        }
        pending = Some((id.to_owned(), path, before));
    }
    pending.ok_or_else(|| "未找到可接续的原控制恢复意图，保留现场".into())
}

fn valid_intent(intent: &Value, state_path: &str) -> Result<bool> {
    let fields: &[&str] = if intent.get("lock").is_some() {
        &[
            "status",
            "owner",
            "state_before",
            "state_after_sha256",
            "lock",
        ]
    } else if intent.get("state_after_sha256").is_some() {
        &["status", "owner", "state_before", "state_after_sha256"]
    } else {
        &["status", "owner", "state_before"]
    };
    json_io::exact(intent, fields, "控制恢复意图")?;
    manifest::validate_binding_shape(&intent["owner"], "原控制器绑定")?;
    manifest::validate_binding_shape(&intent["state_before"], "恢复前阶段状态绑定")?;
    if intent["status"] != "controller_recovery_started"
        || intent["state_before"]["path"] != state_path
        || intent
            .get("state_after_sha256")
            .is_some_and(|value| !value.as_str().is_some_and(|sha| lower_hex(sha, 64)))
    {
        return Err("控制恢复意图内容无效，保留现场".into());
    }
    Ok(intent.get("lock").is_some())
}

fn verify_receipt(run_dir: &Path, path: &Path, intent: &Value) -> Result<()> {
    let result = json_io::read(path)?;
    json_io::exact(
        &result,
        &[
            "status",
            "owner",
            "remote_writes",
            "copy_requires_reconciliation",
            "state_sha256",
        ],
        "控制恢复结果",
    )?;
    identity::validate(&result["owner"], run_dir)?;
    let previous_lock: lock::LockSnapshot = serde_json::from_value(intent["lock"].clone())?;
    if result["status"] != "controller_recovered"
        || result["state_sha256"] != intent["state_after_sha256"]
        || result["remote_writes"] != 0
        || result["copy_requires_reconciliation"] != true
        || previous_lock.is_some_and(|(_, _, owner)| owner != result["owner"])
    {
        return Err("控制恢复结果与原意图不一致，保留现场".into());
    }
    Ok(())
}

fn receipt_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn unchanged(path: &Path, expected: &digest::FileDigest, label: &str) -> Result<()> {
    if digest::file(path)? != *expected {
        return Err(format!("恢复期间{label}发生变化，保留现场").into());
    }
    Ok(())
}
