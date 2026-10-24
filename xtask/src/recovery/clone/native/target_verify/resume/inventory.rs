//! 已完成初始化写入后的库存续作判定；所有读取都绑定登记与当前完整树。
use super::super::{resume_baseline, resume_model, resume_observations, resume_prefix};
use super::*;

pub(super) fn failure(value: &Value) -> Result<bool> {
    json_io::exact(value, super::super::evidence_prepare::FAILURE, "库存原失败")?;
    Ok(value["status"] == "needs_reconciliation"
        && timestamp(&value["at"]).is_ok()
        && value["stage"] == "inventory"
        && value["error_type"] == "InventoryCaptureError"
        && [
            "automatic_retry",
            "automatic_resource_cleanup",
            "fresh_target_initialized",
            "clone_verified",
            "restore_qualified",
        ]
        .iter()
        .all(|field| value[*field] == false))
}

pub(super) fn state(
    backend: &Path,
    target: &Path,
    descriptor: &Value,
    prepared_files: &Value,
    owned_lock: bool,
) -> Result<Value> {
    let files = super::super::tree::snapshot(backend, target, owned_lock)?;
    let prepared = read(backend, &target.join("prepare.json"))?;
    let failure_binding =
        super::super::evidence_prepare::failure_path(backend, target, descriptor)?;
    let failure_path = bound(backend, &failure_binding)?;
    let lock = target.join("initialize.lock");
    let invalid_lock = if owned_lock {
        !lock.is_dir() || std::fs::read_dir(&lock)?.next().is_some()
    } else {
        lock.exists()
    };
    if target.join("initialized.json").exists()
        || target.join("initialized-candidate.json").exists()
        || invalid_lock
        || !names(target, r"^resume-initialize-.*\.(confirmed|started)\.json$")?.is_empty()
        || !names(target, r"^resume-migrate-.*\.json$")?.is_empty()
        || !target.join("initialize.started.json").is_file()
        || !failure_path.is_file()
    {
        return Ok(json!({"resumable":false,"reason":"缺少唯一的已释放 inventory 失败现场"}));
    }
    let failed = read(backend, &failure_path)?;
    if !failure(&failed)? {
        return Ok(json!({"resumable":false,"reason":"失败不是可继续的 inventory 采集失败"}));
    }
    json_io::exact(&prepared, resume_model::PREPARE, "库存 prepare")?;
    let request = read(backend, &bound(backend, descriptor)?)?;
    if prepared["status"] != "fresh_creation_prepared"
        || prepared["request"] != *descriptor
        || prepared["request_sha256"] != digest::plan_hash(&request)?
    {
        return Ok(json!({"resumable":false,"reason":"prepare 请求不属于固定 registration"}));
    }
    if request != read(backend, &target.join("request.json"))? {
        return Ok(json!({"resumable":false,"reason":"固定请求文件已变化"}));
    }
    let baseline = resume_baseline::prepared(backend, target, prepared_files, descriptor)?;
    let creation = super::super::evidence_creation::creation(backend, target, &request)?;
    let reset = super::super::evidence_reset::aggregate(
        backend,
        target,
        &request,
        &prepared["generation"],
    )?;
    let plan = read(backend, &target.join("reset-plan.json"))?;
    let available = resume_observations::file_map(&files)?.into_keys().collect();
    let (attempts, retry_files) = super::inventory_records::records(
        &super::inventory_records::Context {
            backend,
            target,
            failure: &failure_binding,
            failure_at: &failed["at"],
            prepared_files,
            plan: &plan,
            executable: executable(&prepared["generation"]["maintenance"], "reset")?,
            request: &request,
            available: &available,
        },
        None,
    )?;
    let operations = resume_prefix::operations(executable(
        &prepared["generation"]["maintenance"],
        "migrate",
    )?)?;
    let prefix = resume_prefix::prefix(backend, target, &operations["operations"], true)?;
    let mut required =
        super::super::evidence_creation::paths(target, &creation, &reset, &prefix["completed"])?;
    required.insert(
        failure_path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or("库存失败路径无效")?
            .to_owned(),
    );
    required.extend(retry_files);
    required.extend(super::inventory_failure::files(
        backend,
        target,
        "inventory-initial",
        &request,
        &available,
    )?);
    resume_observations::delta(
        backend,
        target,
        &files,
        &baseline,
        &request,
        resume_observations::Delta {
            required: &required,
            rounds: 11 + json_io::array(&prefix["completed"], "完整迁移前缀")?.len() + attempts,
            initialized: None,
        },
    )?;
    check(
        super::super::tree::snapshot(backend, target, owned_lock)? == files,
        "库存续作判定期间目标证据树发生变化",
    )?;
    Ok(
        json!({"resumable":true,"reason":null,"failure":failure_binding,"prepared_files":prepared_files,"baseline_files":baseline,"files":files}),
    )
}

pub(crate) fn confirmed(backend: &Path, target: &Path, initialized: &Value) -> bool {
    verify(backend, target, initialized).is_ok()
}

fn verify(backend: &Path, target: &Path, initialized: &Value) -> Result<()> {
    let files = super::super::tree::snapshot(backend, target, false)?;
    let paths = names(target, r"^resume-initialize-.*\.confirmed\.json$")?;
    check(
        paths.len() == 1
            && names(target, r"^resume-initialize-.*\.started\.json$")?.is_empty()
            && names(target, r"^resume-migrate-.*\.json$")?.is_empty(),
        "库存续作包含挂起、重复或未知结果",
    )?;
    let name = paths.first().ok_or("库存续作包含挂起、重复或未知结果")?;
    let value = read(backend, &target.join(name))?;
    json_io::exact(
        &value,
        &[
            "format_version",
            "kind",
            "attempt",
            "at",
            "intent",
            "failure",
            "prepared_files",
            "initialized",
            "remote_writes",
        ],
        "库存续作确认",
    )?;
    let attempt = attempt(&value["attempt"])?;
    let intent_path = target.join(format!("resume-initialize-{attempt}.intent.json"));
    let intent = read(backend, &intent_path)?;
    json_io::exact(&intent, super::inventory_records::INTENT, "库存续作 intent")?;
    let prepared = read(backend, &target.join("prepare.json"))?;
    json_io::exact(&prepared, resume_model::PREPARE, "库存 prepare")?;
    let failure_binding =
        super::super::evidence_prepare::failure_path(backend, target, &prepared["request"])?;
    let failed = read(backend, &bound(backend, &failure_binding)?)?;
    json_io::exact(initialized, resume_model::INITIALIZED, "初始化结果")?;
    check(
        failure(&failed)?
            && intent["format_version"].as_u64() == Some(1)
            && intent["kind"] == "devex-clone-initialize-resume-intent"
            && intent["attempt"] == attempt
            && intent["failure"] == failure_binding
            && intent["prepared_files"] == value["prepared_files"]
            && zero(&intent["remote_writes"])
            && value["format_version"].as_u64() == Some(1)
            && value["kind"] == "devex-clone-initialize-resume-confirmed"
            && *name == format!("resume-initialize-{attempt}.confirmed.json")
            && value["intent"] == binding(backend, &intent_path)?
            && value["failure"] == failure_binding
            && value["initialized"] == binding(backend, &target.join("initialized.json"))?
            && zero(&value["remote_writes"])
            && read(backend, &target.join("initialized.json"))? == *initialized
            && ordered(&[
                &failed["at"],
                &intent["at"],
                &initialized["completed_at"],
                &value["at"],
            ]),
        "库存续作包含挂起、重复或未知结果",
    )?;
    resume_baseline::prepared(
        backend,
        target,
        &value["prepared_files"],
        &prepared["request"],
    )?;
    let request = read(backend, &bound(backend, &prepared["request"])?)?;
    let available = resume_observations::file_map(&files)?.into_keys().collect();
    super::inventory_records::records(
        &super::inventory_records::Context {
            backend,
            target,
            failure: &failure_binding,
            failure_at: &failed["at"],
            prepared_files: &value["prepared_files"],
            plan: &read(backend, &target.join("reset-plan.json"))?,
            executable: executable(&prepared["generation"]["maintenance"], "reset")?,
            request: &request,
            available: &available,
        },
        Some((attempt, &value["at"])),
    )?;
    check(
        super::super::tree::snapshot(backend, target, false)? == files,
        "库存续作判定期间目标证据树发生变化",
    )
}
