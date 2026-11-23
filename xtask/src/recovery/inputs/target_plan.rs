use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{
    RestoreChecks,
    chain::{self, Chain},
    document::{Document, digest, exact, hash, object, text},
    domain::validate_plan,
    fresh::Fresh,
    independent_b1::Verified,
    product,
    target::TargetRequest,
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
    let value = assemble(
        backend,
        &PlanInputs {
            envelope: &chain.reference.value,
            reference: &chain.reference.value,
            backup: &chain.backup_document,
            comparison: &chain.comparison_document,
            product_arm: chain::source_arm(chain)?,
            arm_input: super::document::binding(&chain.arm.result),
            fresh: fresh_bindings(
                &chain.fresh,
                &result["initialized"],
                &result["target_registration"],
                &result["target_initialized_files"],
                &result["target_environment"],
            ),
            target: &chain.arm.target,
        },
        product_document,
    )?;
    validate_plan(&chain.reference.value, backend)?;
    Ok(value)
}

pub(in crate::recovery) fn capture_independent_b1(
    backend: &Path,
    evidence: &Verified<'_>,
    product_document: &Document,
) -> Result<Value> {
    product::validate_independent_product(evidence, &product_document.value)?;
    let target = evidence.arm.observed_target();
    assemble(
        backend,
        &PlanInputs {
            envelope: &evidence.envelope.value,
            reference: &evidence.reference,
            backup: &evidence.backup_document,
            comparison: &evidence.comparison_document,
            product_arm: evidence.product_arm()?,
            arm_input: evidence.arm.result_binding(),
            fresh: fresh_bindings(
                &evidence.fresh,
                &target.initialized,
                &target.registration,
                &target.files,
                &target.environment,
            ),
            target: &evidence.arm.target_request,
        },
        product_document,
    )
}

fn fresh_bindings(
    fresh: &Fresh,
    initialized: &Value,
    registration: &Value,
    files: &Value,
    environment: &Value,
) -> Value {
    json!({
        "verify": fresh.binding,
        "initialized": initialized,
        "registration": registration,
        "initialized_files": files,
        "environment": environment,
        "inventory": fresh.inventory,
        "ownership_sha256": fresh.ownership_sha256,
    })
}

struct PlanInputs<'a> {
    envelope: &'a Value,
    reference: &'a Value,
    backup: &'a Document,
    comparison: &'a Document,
    product_arm: &'a Value,
    arm_input: Value,
    fresh: Value,
    target: &'a TargetRequest,
}

fn assemble(backend: &Path, inputs: &PlanInputs<'_>, product_document: &Document) -> Result<Value> {
    let name = match inputs.reference["target_side"].as_str() {
        Some("base") => "b0",
        Some("candidate") => "b1",
        _ => return Err("参考计划目标侧无效".into()),
    };
    let comparison_arm_sha256 = hash(inputs.product_arm)?;
    let maintenance_build = object(
        inputs
            .target
            .document
            .value
            .get("maintenance_build")
            .ok_or("新目标请求缺少维护构建")?,
        "维护构建",
    )?;
    let value = json!({
        "format_version": 1,
        "kind": "restore-reference-target-plan",
        "target_side": inputs.reference["target_side"],
        "reference_plan_sha256": hash(inputs.envelope)?,
        "backup_receipt": super::document::binding(inputs.backup),
        "comparison_sources": super::document::binding(inputs.comparison),
        "comparison_arm": name,
        "comparison_arm_sha256": comparison_arm_sha256,
        "arm_input": inputs.arm_input,
        "fresh_target": inputs.fresh,
        "maintenance_execution": {
            "root": inputs.target.execution.to_string_lossy(),
            "binding": inputs.target.execution_binding,
            "build": maintenance_build,
        },
        "product_execution": chain::product_execution(inputs.product_arm)?,
        "product_plan_file": super::document::binding(product_document),
        "product_plan": product_document.value,
        "product_plan_sha256": hash(&product_document.value)?,
    });
    validate_shape(backend, &value, inputs.envelope, inputs.reference)?;
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
    validate_shape(backend, &target.value, &reference.value, &reference.value)?;
    validate_plan(&reference.value, backend)?;
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

fn validate_shape(backend: &Path, value: &Value, envelope: &Value, plan: &Value) -> Result<()> {
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
        || target.get("reference_plan_sha256").and_then(Value::as_str)
            != Some(hash(envelope)?.as_str())
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
    fn independent_b1_target_plan_binds_the_complete_reference_envelope() -> Result<()> {
        let fixture = crate::recovery_inputs_fixture::Fixture::new();
        let (reference, target) = fixture.candidate_plans();
        let reference = document::read(&reference, &fixture.backend, "参考计划")?;
        let (chain, original, product) = verify(&fixture.backend, reference, &target)?;
        let work = Path::new(text(&chain.reference.value["work_dir"], "参考工作目录")?);
        assert!(!work.exists());
        let envelope = json!({
            "format_version":1, "kind":super::super::reference::INDEPENDENT_B1_KIND,
            "reference_plan":chain.reference.value, "arm_input":original["arm_input"],
            "source_export":chain.comparison["source_export"],
        });
        // 已有候选夹具只覆盖目标计划投影，不把普通 ARM 装成独立登记。
        let value = assemble(
            &fixture.backend,
            &PlanInputs {
                envelope: &envelope,
                reference: &chain.reference.value,
                backup: &chain.backup_document,
                comparison: &chain.comparison_document,
                product_arm: chain::source_arm(&chain)?,
                arm_input: original["arm_input"].clone(),
                fresh: original["fresh_target"].clone(),
                target: &chain.arm.target,
            },
            &product,
        )?;
        assert_eq!(value["reference_plan_sha256"], hash(&envelope)?);
        assert_ne!(
            value["reference_plan_sha256"],
            original["reference_plan_sha256"]
        );
        for field in FIELDS
            .into_iter()
            .filter(|field| *field != "reference_plan_sha256")
        {
            assert_eq!(value[field], original[field], "{field}");
        }
        for field in ["source_export", "arm_input"] {
            let mut replaced = envelope.clone();
            replaced[field] = Value::Null;
            assert!(
                validate_shape(&fixture.backend, &value, &replaced, &chain.reference.value)
                    .is_err()
            );
        }
        assert!(!work.exists());
        Ok(())
    }

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
