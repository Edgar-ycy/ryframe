//! 独立 B1 数据绑定必须接续同一已完成外部恢复和原产品运行记录。
use std::{path::Path, time::Duration};

use serde_json::{Value, json};

use super::{BindingsInputOptions, Result, require_bindings};
use crate::recovery::{
    inputs::{
        document::{self, Document, exact},
        domain::timestamp,
        independent_b1::Verified,
        product_record::product_hash,
        target_plan::{evidence::Frame, independent},
    },
    runtime::dataset_authority::reads::Window,
    source::evidence::reference_workflow::CompletedIndependentRestore,
};

pub(super) fn build(
    backend: &Path,
    options: &BindingsInputOptions,
    reference: Document,
) -> Result<Value> {
    let window = Window::direct()?;
    let result = (|| {
        let target = document::read(&options.target_plan, backend, "独立 B1 原目标计划")?;
        let backup = document::read(&options.backup_receipt, backend, "原正式备份收据")?;
        with_completed_target(backend, &reference, &target, &backup, |_, completed| {
            let record = document::read(&options.record, backend, "独立 B1 数据核验记录")?;
            let manifest = completed.manifest();
            verify_advance(&completed.running().value, &record.value, &manifest.value)?;
            let value = json!({"record":record.value, "manifest":manifest.value});
            require_bindings(&value)?;
            record.assert_unchanged()?;
            Ok(value)
        })
    })();
    window.finish(result)
}

/// 在调用方的读取窗口内借用已完成的目标投影和恢复证明。
pub(in crate::recovery) fn with_completed_target<T>(
    backend: &Path,
    reference: &Document,
    target: &Document,
    backup: &Document,
    visit: impl FnOnce(&Verified<'_>, &CompletedIndependentRestore<'_>) -> Result<T>,
) -> Result<T> {
    let frame = Frame::published(backend, reference, target.clone())?;
    let completed = CompletedIndependentRestore::read(backend, reference, target, backup)?;
    let check_work = |selected: &Path, projected: &Value| completed.check_work(selected, projected);
    let (evidence, _) = independent::verify_owned(backend, &frame, &check_work)?;
    let [original_backup, _, _, _, product] = frame.evidence.as_slice() else {
        return Err("独立 B1 数据绑定缺少原目标计划证据".into());
    };
    if document::binding(backup) != document::binding(original_backup)
        || completed.manifest().value != evidence.backup.manifest
    {
        return Err("独立 B1 已完成还原未绑定所选原正式备份".into());
    }
    completed.verify(
        backend,
        evidence.reference_plan(),
        &product_hash(&product.value)?,
        &evidence.arm.target_request.document.value,
    )?;
    let result = visit(&evidence, &completed);
    let after = evidence
        .check()
        .and_then(|()| frame.check())
        .and_then(|()| completed.assert_unchanged());
    match (result, after) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(after)) => {
            Err(format!("{error}；独立 B1 已完成目标的后验同时失败：{after}").into())
        }
    }
}

pub(in crate::recovery) fn verify_advance(
    running: &Value,
    current: &Value,
    manifest: &Value,
) -> Result<()> {
    exact(
        current,
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
        "独立 B1 数据核验记录",
    )?;
    if current["status"] != "data_verified"
        || ["completed_at", "failure"]
            .into_iter()
            .any(|key| !current[key].is_null())
        || ["plan", "plan_hash", "started_at", "recovered_at"]
            .into_iter()
            .any(|key| current[key] != running[key])
    {
        return Err("独立 B1 数据核验记录须接续同一次 Running 及原计划".into());
    }
    let started = timestamp(&current["started_at"], "恢复开始时间")?;
    let verified = timestamp(&current["data_verified_at"], "恢复数据核验时间")?;
    let recovered = timestamp(&current["recovered_at"], "恢复点时间")?;
    let captured = timestamp(&manifest["captured_at"], "原清单采集时间")?;
    if [started, verified, recovered]
        .into_iter()
        .any(|value| !value.timestamp_subsec_nanos().is_multiple_of(1_000))
        || recovered != captured
        || recovered > started
        || verified < started
        || (verified - started).to_std()? > Duration::from_secs(3600)
    {
        return Err("独立 B1 数据核验时间、恢复点或六十分钟期限无效".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_data_binding_checks_the_same_record_and_database_clock() {
        let started = "2026-11-11T04:00:00.123456Z";
        let recovered = "2026-11-11T03:57:15.008900Z";
        let manifest = json!({"captured_at":recovered});
        let running = json!({
            "plan":{"id":"independent-b1"},"plan_hash":"a".repeat(64),"status":"running",
            "started_at":started,"recovered_at":recovered,"data_verified_at":null,
            "completed_at":null,"failure":null
        });
        let mut current = running.clone();
        current["status"] = json!("data_verified");
        current["data_verified_at"] = json!("2026-11-11T05:00:00.123456Z");
        verify_advance(&running, &current, &manifest).unwrap();

        for (key, replacement) in [
            ("data_verified_at", json!("2026-11-11T05:00:00.123457Z")),
            ("data_verified_at", json!("2026-11-11T05:00:00.1234561Z")),
            ("data_verified_at", json!("2026-11-11T04:00:00.123455Z")),
            ("recovered_at", json!("2026-11-11T03:59:00Z")),
            ("started_at", json!("2026-11-11T03:59:00Z")),
            ("plan_hash", json!("b".repeat(64))),
            ("status", json!("failed")),
            ("completed_at", json!("2026-11-11T05:00:00.123456Z")),
            ("failure", json!("意外失败")),
        ] {
            let mut replaced = current.clone();
            replaced[key] = replacement;
            assert!(
                verify_advance(&running, &replaced, &manifest).is_err(),
                "{key}"
            );
        }
        assert!(
            verify_advance(
                &running,
                &current,
                &json!({"captured_at":"2026-11-11T03:57:15.008901Z"})
            )
            .is_err()
        );
    }
}
