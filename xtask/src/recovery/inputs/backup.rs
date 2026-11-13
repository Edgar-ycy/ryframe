use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, FixedOffset};
use serde_json::{Value, json};

use super::{
    document::{Document, digest, exact, integer, object, text},
    domain::{timestamp, validate_local_optional, validate_plan},
    inventory, source,
};
use crate::Result;

pub(crate) fn verify_frozen_source(backend: &Path, backup: &Backup) -> Result<()> {
    let result = &backup.result;
    let mut inventory = object(&backup.manifest, "备份清单")?.clone();
    for field in ["completed_at", "retention_until", "artifacts"] {
        inventory.remove(field);
    }
    let verified = verify_source(
        backend,
        &result["reference_plan"],
        &Value::Object(inventory),
        &result["source_generation"],
        &result["source_export"]["result"],
        false,
    )?;
    if verified["source_export"] != result["source_export"] {
        return Err("备份引用的共享导出与已发布来源不同".into());
    }
    Ok(())
}

pub(crate) fn verify_source(
    backend: &Path,
    plan: &Value,
    inventory: &Value,
    generation: &Value,
    publication: &Value,
    live: bool,
) -> Result<Value> {
    use crate::recovery::{
        clone::native::export,
        source::evidence::{comparison::published, reference_workflow},
    };

    let published = published::verified(backend, publication, None)?;
    if live {
        published.check_live(backend)?;
    }
    let exported = export::frozen::verify(backend, &published.identity["export"])?;
    let read = |descriptor, label| super::document::bound_file(backend, descriptor, label);
    let (_, build) = read(&exported.request["backend_build"], "原源构建")?;
    let (_, stopped) = read(generation, "备份来源停止代次")?;
    let (_, runtime) = read(&stopped.value["source_runtime"], "原来源运行验收")?;
    let (_, quiescence) = read(&stopped.value["runtime_evidence"], "原来源停止观察")?;
    let verified = reference_workflow::request(
        "reference-backup-source",
        &json!({
            "plan":plan, "inventory":inventory, "identity":published.identity,
            "source":published.source,
            "exported":{"request":exported.request,"generation":exported.generation,"inventory":exported.inventory},
            "build":build.value, "descriptor":generation, "runtime":runtime.value, "quiescence":quiescence.value,
        }),
    )?;
    if verified["source_export"] != published.identity
        || verified["source_generation"] != *generation
    {
        return Err("正式备份来源证明或共享导出摘要已变化".into());
    }
    for document in [build, stopped, runtime, quiescence] {
        document.assert_unchanged()?;
    }
    exported.check_proofs(backend, &published.identity["export"])?;
    published.check(backend)?;
    Ok(verified)
}

pub(crate) struct Backup {
    pub(crate) result: Value,
    pub(crate) manifest: Value,
}

pub(crate) fn validate(
    backend: &Path,
    reference_plan: &Value,
    receipt: &Document,
) -> Result<Backup> {
    let receipt_value = exact(
        &receipt.value,
        &[
            "command",
            "plan_sha256",
            "started_at",
            "status",
            "completed_at",
            "result",
        ],
        "备份收据",
    )?;
    if receipt_value.get("command").and_then(Value::as_str) != Some("backup")
        || receipt_value.get("status").and_then(Value::as_str) != Some("completed")
    {
        return Err("目标计划未绑定同一已完成正式备份、来源或目录 ownership".into());
    }
    let started = timestamp(&receipt_value["started_at"], "备份收据开始时间")?;
    let finished = timestamp(&receipt_value["completed_at"], "备份收据完成时间")?;
    if finished < started {
        return Err("备份收据完成时间早于开始时间".into());
    }
    let result = exact(
        receipt_value.get("result").unwrap(),
        &[
            "format_version",
            "kind",
            "reference_plan",
            "manifest",
            "backup_root",
            "artifacts",
            "source_export",
            "source_generation",
        ],
        "正式备份结果",
    )?;
    if result.get("format_version") != Some(&Value::from(2))
        || result.get("kind").and_then(Value::as_str) != Some("restore-reference-backup")
        || integer(result.get("artifacts").unwrap(), "备份产物数量")? == 0
    {
        return Err("正式备份结果版本、类型或产物数量无效".into());
    }
    let original = result.get("reference_plan").unwrap();
    validate_plan(original, backend)?;
    if receipt_value.get("plan_sha256").and_then(Value::as_str)
        != Some(super::document::hash(original)?.as_str())
    {
        return Err("目标计划未绑定同一已完成正式备份、来源或目录 ownership".into());
    }
    let work_dir = PathBuf::from(text(original.get("work_dir").unwrap(), "原始计划工作目录")?);
    let backup_root = PathBuf::from(text(result.get("backup_root").unwrap(), "备份根目录")?);
    validate_local_optional(&backup_root, &backend.join(".local-tests"), "备份根目录")?;
    if backup_root != work_dir.join("backup")
        || receipt.path != work_dir.join("backup.json")
        || !["id", "source", "tools"]
            .into_iter()
            .all(|field| original.get(field) == reference_plan.get(field))
    {
        return Err("目标计划未绑定同一已完成正式备份、来源或目录 ownership".into());
    }
    let owner_path = work_dir.join("reference-owner.json");
    let owner = super::document::read(&owner_path, backend, "参考计划 owner")?;
    let expected_owner = serde_json::json!({
        "format_version": 1,
        "id": original.get("id").unwrap(),
        "plan_sha256": super::document::hash(original)?,
    });
    if owner.value != expected_owner {
        return Err("目标计划未绑定同一已完成正式备份、来源或目录 ownership".into());
    }
    let (_, manifest_document) =
        super::document::bound_file(backend, result.get("manifest").unwrap(), "正式备份清单")?;
    if manifest_document.path != backup_root.join("manifest.json") {
        return Err("正式备份清单位置或产物数量不同".into());
    }
    let manifest = exact(
        &manifest_document.value,
        &[
            "id",
            "scope_id",
            "source_sha",
            "captured_at",
            "quiesced_at",
            "control_schema_fingerprint",
            "tenant_schema_fingerprint",
            "databases",
            "objects",
            "completed_at",
            "retention_until",
            "artifacts",
        ],
        "正式备份清单",
    )?;
    if integer(result.get("artifacts").unwrap(), "备份产物数量")?
        != manifest
            .get("artifacts")
            .and_then(Value::as_array)
            .map_or(0, |items| items.len() as u64)
    {
        return Err("正式备份清单位置或产物数量不同".into());
    }
    inventory::validate(original, &manifest_document.value)?;
    validate_manifest_times(&manifest_document.value, started, finished)?;
    inventory::verify_artifacts(&backup_root, &manifest_document.value)?;
    validate_source_identity(
        backend,
        original,
        result.get("source_export").unwrap(),
        result.get("source_generation").unwrap(),
    )?;
    owner.assert_unchanged()?;
    manifest_document.assert_unchanged()?;
    receipt.assert_unchanged()?;
    Ok(Backup {
        result: receipt_value.get("result").unwrap().clone(),
        manifest: manifest_document.value,
    })
}

fn validate_manifest_times(
    manifest: &Value,
    started: DateTime<FixedOffset>,
    finished: DateTime<FixedOffset>,
) -> Result<()> {
    let captured = timestamp(&manifest["captured_at"], "备份清单采集时间")?;
    let completed = timestamp(&manifest["completed_at"], "备份清单完成时间")?;
    let retention = timestamp(&manifest["retention_until"], "备份清单保留时间")?;
    if [completed, retention]
        .iter()
        .any(|time| !time.timestamp_subsec_nanos().is_multiple_of(1_000))
    {
        return Err("备份清单时间必须符合微秒精度".into());
    }
    // 清单按微秒截断，开始收据保留原始纳秒。
    if completed < captured
        || completed.timestamp_micros() < started.timestamp_micros()
        || completed > finished
    {
        return Err("备份清单采集、完成及收据时间顺序无效".into());
    }
    let minimum = captured
        .checked_add_signed(Duration::days(7))
        .ok_or("备份保留时间溢出")?;
    if retention < minimum {
        return Err("备份清单必须至少保留采集后的 7 天".into());
    }
    Ok(())
}

pub(crate) fn validate_source_identity(
    backend: &Path,
    plan: &Value,
    export: &Value,
    generation: &Value,
) -> Result<()> {
    let export = exact(
        export,
        &[
            "result",
            "origin_attempt",
            "source_registration",
            "source_rebind",
            "source_generation",
            "review_successor",
            "source_request",
            "export",
            "export_sha256",
            "generation_sha256",
            "logical_inventory_sha256",
            "identity_sha256",
        ],
        "备份 source-export 身份",
    )?;
    for field in [
        "result",
        "source_registration",
        "source_rebind",
        "source_generation",
        "review_successor",
        "source_request",
        "export",
    ] {
        super::document::bound_file(backend, export.get(field).unwrap(), "source-export 绑定")?;
    }
    for field in [
        "export_sha256",
        "generation_sha256",
        "logical_inventory_sha256",
        "identity_sha256",
    ] {
        digest(export.get(field).unwrap(), "source-export 摘要")?;
    }
    let mut identity = export.clone();
    let expected = digest(export.get("identity_sha256").unwrap(), "source-export 身份")?;
    identity.remove("identity_sha256");
    if super::document::hash(&Value::Object(identity))? != expected {
        return Err("source-export 内部身份摘要无效".into());
    }
    let request = source::load(
        backend,
        export
            .get("source_request")
            .ok_or("source-export 缺少请求")?,
    )?;
    let projection = super::reference_configuration::SourceProjection::load(backend, &request)?;
    if Some(&projection.source) != plan.get("source")
        || Some(&projection.tools) != plan.get("tools")
    {
        return Err("正式备份与共享导出的库存、配置、构建或运行代次不同".into());
    }
    let (_, generation) =
        super::document::bound_file(backend, generation, "备份 source-generation")?;
    object(&generation.value, "备份 source-generation")?;
    projection.assert_unchanged(backend, &request)?;
    generation.assert_unchanged()?;
    Ok(())
}
