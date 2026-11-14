use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{
    RestoreChecks,
    chain::{self, Chain},
    document::{Document, digest, exact, hash, object, text},
    domain::validate_plan,
    product,
};
use crate::Result;

const FIELDS: [&str; 15] = [
    "format_version",
    "kind",
    "target_side",
    "reference_plan_sha256",
    "backup_receipt",
    "comparison_sources",
    "comparison_arm",
    "comparison_arm_sha256",
    "arm_input",
    "fresh_target",
    "maintenance_execution",
    "product_execution",
    "product_plan_file",
    "product_plan",
    "product_plan_sha256",
];
const FRESH_FIELDS: [&str; 7] = [
    "verify",
    "initialized",
    "registration",
    "initialized_files",
    "environment",
    "inventory",
    "ownership_sha256",
];
const EXECUTION_FIELDS: [&str; 6] = [
    "roots",
    "backend_product_sha",
    "backend_execution_sha",
    "frontend_sha",
    "builds",
    "adapter",
];

pub(crate) fn capture(backend: &Path, chain: &Chain, product_document: &Document) -> Result<Value> {
    product::validate_product(chain, &product_document.value)?;
    let result = object(&chain.arm.result.value, "arm 结果")?;
    let name = match chain.reference.value["target_side"].as_str() {
        Some("base") => "b0",
        Some("candidate") => "b1",
        _ => return Err("参考计划目标侧无效".into()),
    };
    let source_arm = chain::source_arm(chain)?;
    let comparison_arm_sha256 = hash(source_arm)?;
    let maintenance_build = object(
        chain
            .arm
            .target
            .document
            .value
            .get("maintenance_build")
            .ok_or("新目标请求缺少维护构建")?,
        "维护构建",
    )?;
    let fresh = json!({
        "verify": chain.fresh.binding,
        "initialized": result.get("initialized").unwrap(),
        "registration": result.get("target_registration").unwrap(),
        "initialized_files": result.get("target_initialized_files").unwrap(),
        "environment": result.get("target_environment").unwrap(),
        "inventory": chain.fresh.inventory,
        "ownership_sha256": chain.fresh.ownership_sha256,
    });
    let value = json!({
        "format_version": 1,
        "kind": "restore-reference-target-plan",
        "target_side": chain.reference.value["target_side"],
        "reference_plan_sha256": hash(&chain.reference.value)?,
        "backup_receipt": super::document::binding(&chain.backup_document),
        "comparison_sources": super::document::binding(&chain.comparison_document),
        "comparison_arm": name,
        "comparison_arm_sha256": comparison_arm_sha256,
        "arm_input": super::document::binding(&chain.arm.result),
        "fresh_target": fresh,
        "maintenance_execution": {
            "root": chain.arm.target.execution.to_string_lossy(),
            "binding": chain.arm.target.execution_binding,
            "build": maintenance_build,
        },
        "product_execution": chain::product_execution(chain)?,
        "product_plan_file": super::document::binding(product_document),
        "product_plan": product_document.value,
        "product_plan_sha256": hash(&product_document.value)?,
    });
    validate_shape(backend, &value, &chain.reference.value)?;
    Ok(value)
}

pub(crate) fn verify(
    backend: &Path,
    reference: Document,
    target_path: &Path,
) -> Result<(Chain, Value, Document)> {
    verify_with_checks(backend, reference, target_path, RestoreChecks::default())
}

pub(in crate::recovery) fn verify_with_checks(
    backend: &Path,
    reference: Document,
    target_path: &Path,
    checks: RestoreChecks<'_>,
) -> Result<(Chain, Value, Document)> {
    let target = super::document::read(target_path, backend, "目标计划")?;
    validate_shape(backend, &target.value, &reference.value)?;
    let paths = InputPaths::from(&target.value)?;
    let product_document = super::document::read(&paths.product, backend, "产品恢复计划")?;
    let chain = chain::load_verified(
        backend,
        reference,
        [&paths.backup, &paths.comparison, &paths.arm, &paths.fresh],
        text(
            target.value.get("target_side").ok_or("目标计划缺少侧别")?,
            "目标计划侧别",
        )?,
        checks,
    )?;
    let expected = capture(backend, &chain, &product_document)?;
    if expected != target.value {
        return Err("正式恢复目标计划与当前完整来源、ownership 或产品计划不同".into());
    }
    target.assert_unchanged()?;
    product_document.assert_unchanged()?;
    Ok((chain, expected, product_document))
}

fn validate_shape(backend: &Path, value: &Value, plan: &Value) -> Result<()> {
    let target = exact(value, &FIELDS, "目标计划")?;
    let fresh = exact(
        target.get("fresh_target").unwrap(),
        &FRESH_FIELDS,
        "目标计划 fresh_target",
    )?;
    let maintenance = exact(
        target.get("maintenance_execution").unwrap(),
        &["root", "binding", "build"],
        "目标计划维护执行",
    )?;
    let execution = exact(
        target.get("product_execution").unwrap(),
        &EXECUTION_FIELDS,
        "目标计划产品执行",
    )?;
    exact(
        execution.get("roots").unwrap(),
        &["source_backend", "execution_backend", "frontend"],
        "产品执行 roots",
    )?;
    let side = text(target.get("target_side").unwrap(), "目标计划侧别")?;
    let arm = match side {
        "base" => "b0",
        "candidate" => "b1",
        _ => return Err("参考计划目标侧无效".into()),
    };
    if target.get("format_version") != Some(&Value::from(1))
        || target.get("kind").and_then(Value::as_str) != Some("restore-reference-target-plan")
        || plan.get("target_side").and_then(Value::as_str) != Some(side)
        || target.get("comparison_arm").and_then(Value::as_str) != Some(arm)
        || target.get("reference_plan_sha256").and_then(Value::as_str) != Some(hash(plan)?.as_str())
        || target.get("product_plan_sha256").and_then(Value::as_str)
            != Some(hash(target.get("product_plan").unwrap())?.as_str())
    {
        return Err("正式恢复目标计划类型、侧别或摘要不同".into());
    }
    digest(
        target.get("comparison_arm_sha256").unwrap(),
        "比较 arm 摘要",
    )?;
    digest(
        fresh.get("ownership_sha256").unwrap(),
        "fresh ownership 摘要",
    )?;
    for (descriptor, label) in [
        (&target["backup_receipt"], "目标计划备份收据"),
        (&target["comparison_sources"], "目标计划双版本来源"),
        (&target["arm_input"], "目标计划 arm 输入"),
        (&target["product_plan_file"], "目标计划产品恢复文件"),
        (&fresh["verify"], "目标计划目标复核"),
        (&fresh["initialized"], "目标计划初始化结果"),
        (&fresh["registration"], "目标计划初始化登记"),
        (&fresh["initialized_files"], "目标计划初始化文件"),
        (&fresh["environment"], "目标计划初始化环境"),
        (&fresh["inventory"], "目标计划初始化库存"),
        (&maintenance["build"], "目标计划维护构建"),
    ] {
        super::document::bound_file(backend, descriptor, label)?;
    }
    super::comparison::validate_builds(&value["product_execution"])?;
    validate_plan(plan, backend)?;
    Ok(())
}

struct InputPaths {
    backup: PathBuf,
    comparison: PathBuf,
    arm: PathBuf,
    fresh: PathBuf,
    product: PathBuf,
}

impl InputPaths {
    fn from(value: &Value) -> Result<Self> {
        let path = |field: &str| -> Result<PathBuf> {
            let binding = value
                .get(field)
                .ok_or_else(|| format!("目标计划缺少 {field}"))?;
            Ok(super::document::binding_shape(binding, field)?.path)
        };
        Ok(Self {
            backup: path("backup_receipt")?,
            comparison: path("comparison_sources")?,
            arm: path("arm_input")?,
            product: path("product_plan_file")?,
            fresh: super::document::binding_shape(&value["fresh_target"]["verify"], "目标复核")?
                .path,
        })
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::recovery::inputs::{document, inventory::HeldArtifacts};

    #[test]
    fn held_backup_still_validates_the_original_target_plan_and_source_chain() {
        let fixture = crate::recovery_inputs_fixture::Fixture::new();
        let receipt =
            document::read(&fixture.backup_receipt, &fixture.backend, "备份收据").unwrap();
        let accepted = super::super::backup::validate(
            &fixture.backend,
            &fixture.reference_value,
            &receipt,
            None,
        )
        .unwrap();
        let root = Path::new(accepted.result["backup_root"].as_str().unwrap());
        let held = HeldArtifacts::open(root, &accepted.manifest).unwrap();
        let reference =
            || document::read(&fixture.reference_plan, &fixture.backend, "参考计划").unwrap();
        let full = verify(&fixture.backend, reference(), &fixture.target_plan)
            .unwrap()
            .1;
        let verify_held = || {
            verify_with_checks(
                &fixture.backend,
                reference(),
                &fixture.target_plan,
                RestoreChecks {
                    backup: Some(&held),
                    source: None,
                },
            )
        };
        assert_eq!(verify_held().unwrap().1, full);
        held.check_end().unwrap();

        let mut comparison =
            crate::recovery_inputs_support::read_value(&fixture.comparison_sources);
        comparison["source_export"]["origin_attempt"] = json!(0);
        std::fs::write(
            &fixture.comparison_sources,
            serde_json::to_vec(&comparison).unwrap(),
        )
        .unwrap();
        assert!(verify_held().is_err());
        held.check_end().unwrap();
    }
}
