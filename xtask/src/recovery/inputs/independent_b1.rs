//! 复用独立 B1 产品和目标计划的只读权威输入。调用方持有静态读取窗口。
use std::path::Path;

use serde_json::{Map, Value};

use super::{
    backup::{self, Backup},
    comparison,
    document::{self, Document, exact, object, text},
    fresh::{self, Fresh},
    reference,
};
use crate::{
    Result, cli::RestoreInputSide,
    recovery::fixture_control::successor::arm_input::IndependentB1Receipt,
};

pub(in crate::recovery) struct Verified<'a> {
    backend: &'a Path,
    pub(super) envelope: &'a Document,
    pub(super) reference: Value,
    pub(in crate::recovery) backup: Backup,
    pub(super) backup_document: Document,
    pub(in crate::recovery) comparison_document: Document,
    pub(super) comparison: Value,
    pub(in crate::recovery) arm: IndependentB1Receipt<'a>,
    pub(super) fresh: Fresh,
    initialized: Document,
}

impl<'a> Verified<'a> {
    pub(in crate::recovery) fn load(
        backend: &'a Path,
        reference_file: &'a Document,
        [backup, comparison, arm, fresh]: [&Path; 4],
        side: RestoreInputSide,
        output: Option<&Path>,
    ) -> Result<Self> {
        let envelope = independent_reference(&reference_file.value, side)?;
        let (id, work_dir) = reference_location(&envelope["reference_plan"])?;
        if let Some(output) = output {
            reference::validate_publication_path(work_dir, output)?;
        }
        let arm = IndependentB1Receipt::read(backend, arm)?;
        if envelope["arm_input"] != arm.result_binding() {
            return Err("独立 B1 产品计划未绑定原独立 ARM 结果".into());
        }
        require_same_export(arm.source_export(), &envelope["source_export"])?;
        if let Some(output) = output {
            arm.require_disjoint(output)?;
        }
        let reference = reference::project_independent_b1(backend, id, work_dir, &arm)?;
        if reference != envelope["reference_plan"] {
            return Err("独立 B1 参考计划与原 ARM 重新投影不同".into());
        }
        let backup_document = document::read(backup, backend, "备份收据")?;
        let backup = backup::validate(backend, &reference, &backup_document, None)?;
        require_same_export(arm.source_export(), &backup.result["source_export"])?;
        let comparison_document = document::read(comparison, backend, "双版本来源清单")?;
        let comparison = comparison::validate(backend, &comparison_document)?;
        require_same_export(arm.source_export(), &comparison["source_export"])?;
        let (initialized_binding, initialized) = arm.initialized()?;
        let fresh = fresh::validate(
            backend,
            fresh,
            initialized_binding,
            &initialized.value,
            &arm.target_request.document.value,
        )?;
        Ok(Self {
            backend,
            envelope: reference_file,
            reference,
            backup,
            backup_document,
            comparison_document,
            comparison,
            arm,
            fresh,
            initialized,
        })
    }

    pub(super) fn product_arm(&self) -> Result<&Value> {
        comparison::arm(&self.comparison, "candidate")
    }

    pub(in crate::recovery) fn check(&self) -> Result<()> {
        let (id, work_dir) = reference_location(&self.reference)?;
        if reference::project_independent_b1(self.backend, id, work_dir, &self.arm)?
            != self.reference
        {
            return Err("独立 B1 参考计划在核验期间变化".into());
        }
        self.initialized.assert_unchanged()?;
        self.arm.check()?;
        self.envelope.assert_unchanged()?;
        self.backup_document.assert_unchanged()?;
        self.comparison_document.assert_unchanged()
    }
}

fn reference_location(reference: &Value) -> Result<(&str, &Path)> {
    let id = text(&reference["id"], "独立 B1 参考计划 ID")?;
    let work_dir = Path::new(text(&reference["work_dir"], "独立 B1 参考计划工作目录")?);
    Ok((id, work_dir))
}

fn independent_reference(value: &Value, side: RestoreInputSide) -> Result<&Map<String, Value>> {
    let envelope = exact(
        value,
        &[
            "format_version",
            "kind",
            "reference_plan",
            "arm_input",
            "source_export",
        ],
        "独立 B1 参考计划封装",
    )?;
    if !reference::is_independent_b1(value) || envelope["format_version"] != 1 {
        return Err("独立 B1 参考计划封装类型或版本无效".into());
    }
    let stored = object(&envelope["reference_plan"], "独立 B1 参考计划")?;
    if side != RestoreInputSide::Candidate
        || stored.get("target_side").and_then(Value::as_str) != Some("candidate")
    {
        return Err("独立 B1 产品计划只允许 candidate 侧".into());
    }
    Ok(envelope)
}

pub(super) fn require_same_export(original: &Value, observed: &Value) -> Result<()> {
    if original != observed {
        return Err("独立 B1 产品计划未绑定同一原共享导出".into());
    }
    Ok(())
}
