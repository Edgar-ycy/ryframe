use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{
    document::{Document, digest, exact, integer, object, text},
    domain::{timestamp, validate_local_optional, validate_plan},
    inventory, source,
};
use crate::Result;

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
    timestamp(receipt_value.get("started_at").unwrap(), "备份收据时间")?;
    timestamp(receipt_value.get("completed_at").unwrap(), "备份收据时间")?;
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

fn validate_source_identity(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        recovery_inputs_fixture::Fixture,
        recovery_inputs_support::{binding, hash, read_value},
    };
    use serde_json::json;

    #[test]
    fn source_identity_rejects_changed_reference_and_private_credentials_without_writes() {
        let fixture = Fixture::new();
        let receipt = read_value(&fixture.backup_receipt);
        let export = &receipt["result"]["source_export"];
        let generation = &receipt["result"]["source_generation"];
        validate_source_identity(
            &fixture.backend,
            &fixture.reference_value,
            export,
            generation,
        )
        .unwrap();
        for (section, field, replacement) in [
            ("source", "scope_id", json!("another-source")),
            (
                "tools",
                "mysql",
                json!({"path":fixture.root.join("other.exe"),"sha256":"a".repeat(64)}),
            ),
            (
                "tools",
                "mysqldump",
                json!({"path":fixture.reference_value["tools"]["mysqldump"]["path"],"sha256":"a".repeat(64)}),
            ),
            (
                "source",
                "s3",
                fixture.reference_value["target"]["s3"].clone(),
            ),
        ] {
            let mut changed = fixture.reference_value.clone();
            changed[section][field] = replacement;
            assert!(
                validate_source_identity(&fixture.backend, &changed, export, generation).is_err()
            );
            assert!(!fixture.output.exists());
        }
        let path = fixture.root.join("source-request.json");
        let original = std::fs::read(&path).unwrap();
        let mut request = read_value(&path);
        request["source"]["s3"]["secret_key"] = json!("different-private-secret");
        std::fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
        let mut changed = export.clone();
        changed["source_request"] = binding(&path);
        changed.as_object_mut().unwrap().remove("identity_sha256");
        changed["identity_sha256"] = json!(hash(&changed));
        let before = std::fs::read(&path).unwrap();
        let error = validate_source_identity(
            &fixture.backend,
            &fixture.reference_value,
            &changed,
            generation,
        )
        .err()
        .unwrap()
        .to_string();
        assert!(error.contains("object_storage.secret_key与绑定私有请求不一致"));
        assert!(!error.contains("different-private-secret"));
        assert!(!error.contains("source-secret"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!fixture.output.exists());
        std::fs::write(path, original).unwrap();
    }
}
