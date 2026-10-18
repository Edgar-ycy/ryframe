use std::path::Path;

use serde_json::{Value, json};

use super::{
    document::{digest, exact, hash, text},
    domain::{hash_40, identifier, scope_identifier, timestamp},
    target_plan,
};
use crate::{Result, cli::BindingsInputOptions};

pub(crate) fn build(backend: &Path, options: &BindingsInputOptions) -> Result<Value> {
    let reference = super::document::read(&options.reference_plan, backend, "参考计划")?;
    let (chain, target, _product) = target_plan::verify(backend, reference, &options.target_plan)?;
    let backup = super::document::read(&options.backup_receipt, backend, "指定备份收据")?;
    if super::document::binding(&backup) != super::document::binding(&chain.backup_document) {
        return Err("指定备份收据与目标计划绑定不同".into());
    }
    let record = super::document::read(&options.record, backend, "数据核验记录")?;
    let record_value = exact(
        &record.value,
        &[
            "plan",
            "plan_hash",
            "status",
            "started_at",
            "data_verified_at",
            "completed_at",
            "recovered_at",
            "failure",
        ],
        "数据核验记录",
    )?;
    let product = target.get("product_plan").ok_or("目标计划缺少产品计划")?;
    if record_value.get("plan") != Some(product)
        || record_value.get("plan_hash").and_then(Value::as_str) != Some(hash(product)?.as_str())
    {
        return Err("运行绑定的产品计划、备份或数据核验记录不一致".into());
    }
    let (_, manifest) = super::document::bound_file(
        backend,
        chain
            .backup
            .result
            .get("manifest")
            .ok_or("备份结果缺少清单")?,
        "正式备份清单",
    )?;
    if target.get("backup_receipt") != Some(&super::document::binding(&chain.backup_document))
        || chain.backup.result.get("manifest") != Some(&super::document::binding(&manifest))
    {
        return Err("运行绑定的产品计划、备份或数据核验记录不一致".into());
    }
    let value = json!({
        "record": record_value,
        "manifest": manifest.value,
    });
    require_bindings(&value)?;
    record.assert_unchanged()?;
    manifest.assert_unchanged()?;
    backup.assert_unchanged()?;
    Ok(value)
}

fn require_bindings(value: &Value) -> Result<()> {
    let root = exact(value, &["record", "manifest"], "恢复 bindings")?;
    let record = super::document::object(root.get("record").unwrap(), "恢复记录")?;
    let manifest = super::document::object(root.get("manifest").unwrap(), "恢复清单")?;
    let plan = super::document::object(
        record.get("plan").ok_or("恢复记录缺少 plan")?,
        "产品恢复计划",
    )?;
    if record.get("status").and_then(Value::as_str) != Some("data_verified")
        || plan.get("backup_id") != manifest.get("id")
    {
        return Err("运行产物必须绑定已完成数据校验的恢复演练".into());
    }
    digest(record.get("plan_hash").unwrap(), "恢复记录 plan hash")?;
    hash_40(manifest.get("source_sha").unwrap(), "恢复清单源码")?;
    hash_40(plan.get("frontend_sha").unwrap(), "恢复计划前端源码")?;
    for field in ["id", "backup_id"] {
        identifier(text(plan.get(field).unwrap(), "恢复计划 ID")?)?;
    }
    scope_identifier(text(plan.get("scope_id").unwrap(), "恢复 scope")?)?;
    timestamp(record.get("data_verified_at").unwrap(), "恢复数据校验时间")?;
    Ok(())
}
