use std::path::Path;

use serde_json::Value;

use super::{
    RestoreChecks,
    arm::{self, Arm, Fresh},
    backup::{self, Backup},
    comparison,
    document::{Document, exact, text},
    domain::validate_plan,
};
use crate::Result;

pub(crate) struct Chain {
    pub(crate) reference: Document,
    pub(crate) backup_document: Document,
    pub(crate) comparison_document: Document,
    pub(crate) backup: Backup,
    pub(crate) comparison: Value,
    pub(crate) arm: Arm,
    pub(crate) fresh: Fresh,
}

#[cfg(test)]
pub(crate) fn load(
    backend: &Path,
    reference_path: &Path,
    backup_path: &Path,
    comparison_path: &Path,
    arm_path: &Path,
    fresh_path: &Path,
    side: &str,
) -> Result<Chain> {
    let reference = super::document::read(reference_path, backend, "参考计划")?;
    load_with_reference(
        backend,
        reference,
        backup_path,
        comparison_path,
        arm_path,
        fresh_path,
        side,
    )
}

pub(crate) fn load_with_reference(
    backend: &Path,
    reference: Document,
    backup_path: &Path,
    comparison_path: &Path,
    arm_path: &Path,
    fresh_path: &Path,
    side: &str,
) -> Result<Chain> {
    load_verified(
        backend,
        reference,
        [backup_path, comparison_path, arm_path, fresh_path],
        side,
        RestoreChecks::default(),
    )
}

pub(super) fn load_verified(
    backend: &Path,
    reference: Document,
    [backup_path, comparison_path, arm_path, fresh_path]: [&Path; 4],
    side: &str,
    checks: RestoreChecks<'_>,
) -> Result<Chain> {
    validate_plan(&reference.value, backend)?;
    if reference.value.get("target_side").and_then(Value::as_str) != Some(side) {
        return Err("产品计划侧别与参考计划不同".into());
    }
    let backup_document = super::document::read(backup_path, backend, "备份收据")?;
    let backup = backup::validate(backend, &reference.value, &backup_document, checks.backup)?;
    let comparison_document = super::document::read(comparison_path, backend, "双版本来源清单")?;
    let comparison = comparison::validate(backend, &comparison_document)?;
    let (arm, fresh) = arm::load_with_source(backend, arm_path, fresh_path, side, checks.source)?;
    validate_links(&reference.value, &backup, &comparison, &arm, &fresh)?;
    Ok(Chain {
        reference,
        backup_document,
        comparison_document,
        backup,
        comparison,
        arm,
        fresh,
    })
}

pub(crate) fn validate_links(
    reference: &Value,
    backup: &Backup,
    comparison: &Value,
    arm: &Arm,
    _fresh: &Fresh,
) -> Result<()> {
    let export = comparison
        .get("source_export")
        .ok_or("双版本来源缺少 source-export")?;
    let result = arm.result.value.as_object().ok_or("arm 结果必须为对象")?;
    if backup.result.get("source_export") != Some(export)
        || result.get("source_export_result") != export.get("result")
        || result.get("source_export") != export.get("export")
        || result.get("source_generation") != export.get("source_generation")
    {
        return Err("产品计划未绑定同一备份、比较来源或 fresh 目标".into());
    }
    let target = reference.get("target").ok_or("参考计划缺少目标")?;
    let projection = super::reference_configuration::TargetProjection::load(&arm.target)?;
    if projection.target
        != serde_json::json!({
            "scope_id": target.get("scope_id").unwrap(),
            "s3": target.get("s3").unwrap(),
            "databases": target.get("databases").unwrap(),
        })
    {
        return Err("产品计划未绑定同一备份、比较来源或 fresh 目标".into());
    }
    for field in ["runtime_dir", "api_url", "worker_ready_url", "frontend_url"] {
        if target.get(field) != arm.target.selected.get(field) {
            return Err("产品计划未绑定同一备份、比较来源或 fresh 目标".into());
        }
    }
    if arm.result.value.get("target_side").and_then(Value::as_str)
        != reference.get("target_side").and_then(Value::as_str)
    {
        return Err("产品计划侧别与 arm 不同".into());
    }
    projection.assert_unchanged(&arm.target)?;
    Ok(())
}

pub(crate) fn source_arm(chain: &Chain) -> Result<&Value> {
    comparison::arm(
        &chain.comparison,
        text(
            chain
                .reference
                .value
                .get("target_side")
                .ok_or("参考计划缺少目标侧")?,
            "参考计划目标侧",
        )?,
    )
}

pub(crate) fn frontend_sha(chain: &Chain) -> Result<String> {
    let value = source_arm(chain)?
        .get("sources")
        .and_then(|value| value.get("frontend"))
        .and_then(|value| value.get("source"))
        .and_then(|value| value.get("snapshot"))
        .and_then(|value| value.get("head"))
        .and_then(Value::as_str)
        .ok_or("比较来源缺少前端 SHA")?;
    Ok(value.to_owned())
}

pub(super) fn product_execution(arm: &Value) -> Result<Value> {
    let roots = exact(
        arm.get("roots").ok_or("比较 source arm 缺少 roots")?,
        &["source_backend", "execution_backend", "frontend"],
        "产品执行来源 roots",
    )?;
    Ok(serde_json::json!({
        "roots": roots,
        "backend_product_sha": arm["sources"]["backend"]["source"]["snapshot"]["head"],
        "backend_execution_sha": arm["execution_sources"]["backend"]["source"]["snapshot"]["head"],
        "frontend_sha": arm["sources"]["frontend"]["source"]["snapshot"]["head"],
        "builds": arm.get("builds").ok_or("比较 source arm 缺少构建")?,
        "adapter": arm.get("adapter").ok_or("比较 source arm 缺少适配")?,
    }))
}
