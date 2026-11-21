use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{identity, lock, manifest, state};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{digest, json as json_io, resources::common},
};

pub(crate) fn run(backend: &Path, run_dir: &Path, owner_binding: &Path) -> Result<Value> {
    run_with_result_publisher(backend, run_dir, owner_binding, |path, value| {
        json_io::publish_new(path, value, "控制恢复结果")
    })
}

struct Intent {
    id: String,
    path: PathBuf,
    bytes: digest::FileDigest,
    value: Value,
}

struct BoundOwner {
    value: Value,
    bytes: Option<digest::FileDigest>,
    number: Option<u64>,
    historical_record: Option<(PathBuf, Option<digest::FileDigest>)>,
}

pub(crate) fn run_with_result_publisher(
    backend: &Path,
    run_dir: &Path,
    owner_binding: &Path,
    publish: impl FnOnce(&Path, &Value) -> Result<()>,
) -> Result<Value> {
    validate_local_test_path(run_dir, backend, LocalTestPathKind::ExistingDirectory)
        .map_err(|error| format!("复制运行目录无效：{error}"))?;
    let state_path = run_dir.join("state.json");
    let before_guard = state_without_lock(run_dir, &state_path)?;
    let binding_kind =
        if before_guard.is_some() && owner_binding == run_dir.join("run.lock/owner.json") {
            LocalTestPathKind::OutputFile
        } else {
            LocalTestPathKind::ExistingFile
        };
    validate_local_test_path(owner_binding, backend, binding_kind)
        .map_err(|error| format!("控制器绑定无效：{error}"))?;
    let guard = if before_guard.is_some() {
        lock::ProcessGuard::acquire_existing(run_dir)?
    } else {
        lock::ProcessGuard::acquire(run_dir)?
    };
    let state_before = digest::file(&state_path)?;
    let saved = state::load(run_dir, false)?;
    let running = state::running_attempt(&saved)?;
    let original_lock = lock::lock_snapshot(run_dir)?;
    if before_guard.is_some() != (running.is_none() && original_lock.is_none())
        || before_guard.is_some_and(|before| before != state_before)
    {
        return Err("获取控制互斥后恢复状态或控制锁变化，保留现场".into());
    }
    let (owner, intent, state_bytes) = if let Some(running) = running {
        begin_recovery(
            run_dir,
            owner_binding,
            running,
            &state_before,
            &original_lock,
        )?
    } else {
        let attempt = interrupted_attempt(&saved)?;
        let intent = pending_intent(run_dir, &digest::plan_hash(&saved)?)?;
        let owner = resume_owner(run_dir, owner_binding, attempt, &original_lock, &intent)?;
        (owner, intent, state_before)
    };
    let check = |snapshot: &lock::LockSnapshot| {
        guard.check()?;
        lock::check_recovery_snapshot(run_dir, snapshot)?;
        check_owner(run_dir, owner_binding, &owner, snapshot.is_some())?;
        unchanged(&state_path, &state_bytes, "阶段状态")?;
        unchanged(&intent.path, &intent.bytes, "原控制恢复意图")
    };
    check(&original_lock)?;
    let result_path = run_dir.join(format!("recovered-{}.json", intent.id));
    let result = json!({
        "status": "controller_recovered",
        "owner": owner.value,
        "remote_writes": 0,
        "copy_requires_reconciliation": true,
        "state_sha256": intent.value["state_after_sha256"],
    });
    if original_lock.is_some() {
        reject_existing_result(&result_path, &result, false)?;
        lock::recover_dead(run_dir, &owner.value, Some(&original_lock))?;
        check(&None)?;
    }
    reject_existing_result(&result_path, &result, true)?;
    publish(&result_path, &result)?;
    check(&None)?;
    verify_exact_result(&result_path, &result)?;
    Ok(result)
}

fn state_without_lock(run_dir: &Path, state_path: &Path) -> Result<Option<digest::FileDigest>> {
    if receipt_exists(&run_dir.join("run.lock"))? {
        return Ok(None);
    }
    let before = digest::file(state_path)?;
    let saved = state::load(run_dir, false)?;
    Ok(state::running_attempt(&saved)?.is_none().then_some(before))
}

fn begin_recovery(
    run_dir: &Path,
    owner_binding: &Path,
    running: &Value,
    state_before: &digest::FileDigest,
    original_lock: &lock::LockSnapshot,
) -> Result<(BoundOwner, Intent, digest::FileDigest)> {
    let owner = existing_owner(run_dir, owner_binding, running, false)?;
    verify_dead_owner(run_dir, &owner.value)?;
    if original_lock
        .as_ref()
        .is_some_and(|(_, _, saved)| saved != &owner.value)
    {
        return Err("现有控制锁属于其他控制器，不能接管".into());
    }
    let descriptor =
        manifest::binding(owner_binding, owner.bytes.as_ref().ok_or("缺少控制器绑定")?)?;
    let state_path = run_dir.join("state.json");
    let id = uuid::Uuid::new_v4().simple().to_string();
    let path = run_dir.join(format!("recovery-{id}.intent.json"));
    let mut published = None;
    state::interrupt_owned(run_dir, &owner.value, owner.number, |updated| {
        let value = json!({
            "status": "controller_recovery_started",
            "owner": descriptor,
            "state_before": manifest::binding(&state_path, state_before)?,
            "state_after_sha256": digest::plan_hash(updated)?,
            "lock": original_lock,
        });
        json_io::publish_new(&path, &value, "控制恢复意图")?;
        let intent = read_intent(id, path)?;
        if intent.value != value {
            return Err("刚发布的控制恢复意图发生变化，保留现场".into());
        }
        published = Some(intent);
        check_owner(run_dir, owner_binding, &owner, original_lock.is_some())?;
        lock::check_recovery_snapshot(run_dir, original_lock)?;
        unchanged(&state_path, state_before, "阶段状态")
    })?;
    let intent = published.ok_or("缺少本次控制恢复意图")?;
    let state_bytes = digest::file(&state_path)?;
    if intent.value["state_after_sha256"] != digest::plan_hash(&state::load(run_dir, false)?)? {
        return Err("已提交阶段状态与控制恢复意图不符，保留现场".into());
    }
    Ok((owner, intent, state_bytes))
}

fn owner_from_binding(
    run_dir: &Path,
    owner_binding: &Path,
    attempt: &Value,
    retry: bool,
) -> Result<(Value, Option<u64>)> {
    if owner_binding == run_dir.join("run.lock/owner.json") {
        return Ok((json_io::read(owner_binding)?, None));
    }
    let number = attempt["number"].as_u64().ok_or("控制器阶段序号无效")?;
    if owner_binding != state::controller_path(run_dir, number) {
        return Err("旧控制器不能恢复新的运行阶段".into());
    }
    let (_, owner) = if retry {
        interrupted_controller(run_dir, number, attempt)?
    } else {
        state::controller_record(run_dir, number, attempt)?
    };
    Ok((owner, Some(number)))
}

fn existing_owner(
    run_dir: &Path,
    owner_binding: &Path,
    attempt: &Value,
    retry: bool,
) -> Result<BoundOwner> {
    let bytes = digest::file(owner_binding)?;
    let (value, number) = owner_from_binding(run_dir, owner_binding, attempt, retry)?;
    unchanged(owner_binding, &bytes, "控制器绑定")?;
    Ok(BoundOwner {
        value,
        bytes: Some(bytes),
        number,
        historical_record: None,
    })
}

fn resume_owner(
    run_dir: &Path,
    owner_binding: &Path,
    attempt: &Value,
    current_lock: &lock::LockSnapshot,
    intent: &Intent,
) -> Result<BoundOwner> {
    let historical_lock: lock::LockSnapshot = serde_json::from_value(intent.value["lock"].clone())?;
    if current_lock.is_some() && current_lock != &historical_lock {
        return Err("控制恢复意图与当前控制器或控制锁不一致，保留现场".into());
    }
    let owner_path = run_dir.join("run.lock/owner.json");
    let owner = if current_lock.is_none() && owner_binding == owner_path {
        let (_, descriptor, owner) = historical_lock.as_ref().ok_or("原控制恢复意图没有控制锁")?;
        if *descriptor != intent.value["owner"] {
            return Err("原控制锁的持有人绑定与控制恢复意图不一致，保留现场".into());
        }
        historical_owner(run_dir, attempt, owner)?
    } else {
        let owner = existing_owner(run_dir, owner_binding, attempt, true)?;
        let descriptor =
            manifest::binding(owner_binding, owner.bytes.as_ref().ok_or("缺少控制器绑定")?)?;
        if intent.value["owner"] != descriptor {
            return Err("控制恢复意图与当前控制器或控制锁不一致，保留现场".into());
        }
        owner
    };
    if let Some((_, descriptor, saved_owner)) = historical_lock {
        manifest::validate_binding_shape(&descriptor, "原控制锁持有人绑定")?;
        if descriptor["path"] != manifest::path_text(&owner_path, "控制锁持有人路径")?
            || saved_owner != owner.value
        {
            return Err("原控制锁与控制恢复意图不一致，保留现场".into());
        }
    }
    Ok(owner)
}

fn historical_owner(run_dir: &Path, attempt: &Value, owner: &Value) -> Result<BoundOwner> {
    let number = attempt["number"].as_u64().ok_or("控制器阶段序号无效")?;
    let path = state::controller_path(run_dir, number);
    let bytes = if receipt_exists(&path)? {
        let before = digest::file(&path)?;
        let (descriptor, recorded_owner) = interrupted_controller(run_dir, number, attempt)?;
        if descriptor != manifest::binding(&path, &before)? || recorded_owner != *owner {
            return Err("已登记持久控制器与原控制锁不一致，保留现场".into());
        }
        Some(before)
    } else {
        None
    };
    Ok(BoundOwner {
        value: owner.clone(),
        bytes: None,
        number: None,
        historical_record: Some((path, bytes)),
    })
}

fn interrupted_controller(run_dir: &Path, number: u64, attempt: &Value) -> Result<(Value, Value)> {
    let mut running = attempt.clone();
    running["status"] = json!("running");
    running["finished_at"] = Value::Null;
    running["error_type"] = Value::Null;
    state::controller_record(run_dir, number, &running)
}

fn check_owner(
    run_dir: &Path,
    owner_binding: &Path,
    owner: &BoundOwner,
    lock_present: bool,
) -> Result<()> {
    if let Some(bytes) = &owner.bytes
        && (lock_present || owner.number.is_some())
    {
        unchanged(owner_binding, bytes, "控制器绑定")?;
    }
    if let Some((path, bytes)) = &owner.historical_record {
        match bytes {
            Some(bytes) => unchanged(path, bytes, "已登记持久控制器绑定")?,
            None if receipt_exists(path)? => {
                return Err("恢复期间出现其他持久控制器绑定，保留现场".into());
            }
            None => {}
        }
    }
    verify_dead_owner(run_dir, &owner.value)
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

fn pending_intent(run_dir: &Path, after_sha256: &str) -> Result<Intent> {
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
        let intent = read_intent(id.to_owned(), entry.path())?;
        if !valid_intent(&intent.value, &state_path)?
            || intent.value["state_after_sha256"] != after_sha256
        {
            continue;
        }
        if pending.is_some() {
            return Err("存在多份对应当前阶段状态的控制恢复意图，保留现场".into());
        }
        pending = Some(intent);
    }
    pending.ok_or_else(|| "未找到可接续的原控制恢复意图，保留现场".into())
}

fn read_intent(id: String, path: PathBuf) -> Result<Intent> {
    let file = digest::open(&path)?;
    let mut source = digest::HashingIo::new(file.take(json_io::MAX_JSON_BYTES + 1));
    let mut raw = Vec::new();
    source.read_to_end(&mut raw)?;
    let (_, bytes) = source.finish();
    if bytes.bytes > json_io::MAX_JSON_BYTES {
        return Err("控制恢复意图超过 16 MiB，保留现场".into());
    }
    let text = std::str::from_utf8(&raw).map_err(|_| "控制恢复意图必须为 UTF-8 JSON")?;
    let value = json_io::parse_fragment(text, "控制恢复意图")?;
    unchanged(&path, &bytes, "原控制恢复意图")?;
    Ok(Intent {
        id,
        path,
        bytes,
        value,
    })
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

fn reject_existing_result(path: &Path, expected: &Value, lock_absent: bool) -> Result<()> {
    if receipt_exists(path)? {
        verify_exact_result(path, expected)?;
        if lock_absent {
            return Err("控制恢复结果已存在且与原意图一致，保留现场".into());
        }
        return Err("控制恢复结果已存在，保留现场".into());
    }
    Ok(())
}

fn verify_exact_result(path: &Path, expected: &Value) -> Result<()> {
    let mut encoded = serde_json::to_vec_pretty(expected)?;
    encoded.push(b'\n');
    let file = digest::open(path)?;
    let handle = same_file::Handle::from_file(file.try_clone()?)?;
    let mut actual = Vec::with_capacity(encoded.len());
    file.take(encoded.len() as u64 + 1)
        .read_to_end(&mut actual)?;
    let metadata = fs::symlink_metadata(path)?;
    if actual != encoded
        || !metadata.is_file()
        || digest::linked(&metadata)
        || common::link_count_for(path, &metadata)? != 1
        || same_file::Handle::from_path(path)? != handle
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
