use std::path::Path;

use chrono::{DateTime, Duration, SecondsFormat, Timelike, Utc};
use serde_json::{Map, Value, json};

use super::{
    backup::{self, Backup},
    chain::{self, Chain},
    comparison,
    document::{self, Document, array, exact, object, text},
    domain::{canonical_endpoint, identifier, product_fields, scope_identifier, timestamp},
    fresh, reference,
};
use crate::{
    Result,
    cli::{ProductInputOptions, RestoreInputSide},
    recovery::{
        fixture_control::successor::arm_input::IndependentB1Receipt,
        runtime::dataset_authority::reads::Window,
    },
};

pub(crate) fn build(backend: &Path, options: &ProductInputOptions) -> Result<Value> {
    identifier(&options.id)?;
    let fault = parse_fault_time(&options.fault_at)?;
    let reference = document::read(&options.reference_plan, backend, "参考计划")?;
    if reference.value["kind"] == reference::INDEPENDENT_B1_KIND {
        let window = Window::direct()?;
        let result = reference
            .assert_unchanged()
            .and_then(|()| build_independent_b1(backend, options, &reference, fault));
        return window.finish(result);
    }
    let chain = chain::load_with_reference(
        backend,
        reference,
        &options.backup_receipt,
        &options.comparison_sources,
        &options.arm_input,
        &options.fresh_target_verify,
        options.side.as_str(),
    )?;
    validate_recovery_point(&chain.backup, fault)?;
    let plan = project(&chain, &options.id, &options.fault_at)?;
    chain.reference.assert_unchanged()?;
    chain.backup_document.assert_unchanged()?;
    chain.comparison_document.assert_unchanged()?;
    chain.arm.result.assert_unchanged()?;
    chain.arm.request.assert_unchanged()?;
    chain.arm.manifest.assert_unchanged()?;
    chain.arm.source_request.assert_unchanged()?;
    Ok(plan)
}

fn build_independent_b1(
    backend: &Path,
    options: &ProductInputOptions,
    reference_file: &Document,
    fault: DateTime<Utc>,
) -> Result<Value> {
    let envelope = independent_reference(&reference_file.value, options.side)?;
    let id = text(&envelope["reference_plan"]["id"], "独立 B1 参考计划 ID")?;
    let work_dir = Path::new(text(
        &envelope["reference_plan"]["work_dir"],
        "独立 B1 参考计划工作目录",
    )?);
    if let Some(publication) = &options.publication {
        reference::validate_publication_path(work_dir, &publication.output)?;
    }
    let arm = IndependentB1Receipt::read(backend, &options.arm_input)?;
    if envelope["arm_input"] != arm.result_binding() {
        return Err("独立 B1 产品计划未绑定原独立 ARM 结果".into());
    }
    require_same_export(arm.source_export(), &envelope["source_export"])?;
    if let Some(publication) = &options.publication {
        arm.require_disjoint(&publication.output)?;
    }
    let reference = reference::project_independent_b1(backend, id, work_dir, &arm)?;
    if reference != envelope["reference_plan"] {
        return Err("独立 B1 参考计划与原 ARM 重新投影不同".into());
    }
    let backup_file = document::read(&options.backup_receipt, backend, "备份收据")?;
    let backup = backup::validate(backend, &reference, &backup_file, None)?;
    validate_recovery_point(&backup, fault)?;
    require_same_export(arm.source_export(), &backup.result["source_export"])?;
    let comparison_file = document::read(&options.comparison_sources, backend, "双版本来源清单")?;
    let comparison = comparison::validate(backend, &comparison_file)?;
    require_same_export(arm.source_export(), &comparison["source_export"])?;
    let b1 = comparison::arm(&comparison, options.side.as_str())?;
    let frontend_sha = text(
        &b1["sources"]["frontend"]["source"]["snapshot"]["head"],
        "比较来源前端 SHA",
    )?;
    let (initialized_binding, initialized) = arm.initialized()?;
    let target = &arm.target_request;
    fresh::validate(
        backend,
        &options.fresh_target_verify,
        initialized_binding,
        &initialized.value,
        &target.document.value,
    )?;
    let plan = project_inputs(
        &reference,
        &target.selected["worker_ready_url"],
        frontend_sha,
        &options.id,
        &options.fault_at,
    )?;
    if reference::project_independent_b1(backend, id, work_dir, &arm)? != reference {
        return Err("独立 B1 参考计划在产品计划投影期间变化".into());
    }
    initialized.assert_unchanged()?;
    arm.check()?;
    reference_file.assert_unchanged()?;
    backup_file.assert_unchanged()?;
    comparison_file.assert_unchanged()?;
    Ok(plan)
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
    if envelope["kind"] != reference::INDEPENDENT_B1_KIND || envelope["format_version"] != 1 {
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

fn require_same_export(original: &Value, observed: &Value) -> Result<()> {
    if original != observed {
        return Err("独立 B1 产品计划未绑定同一原共享导出".into());
    }
    Ok(())
}

pub(super) fn validate_product(chain: &Chain, value: &Value) -> Result<()> {
    let plan = exact(value, &product_fields(), "产品恢复计划")?;
    let id = text(&plan["id"], "产品恢复 ID")?;
    identifier(id)?;
    identifier(text(&plan["backup_id"], "产品备份 ID")?)?;
    scope_identifier(text(&plan["scope_id"], "产品 scope")?)?;
    let fault_at = text(&plan["fault_at"], "产品故障时间")?;
    validate_recovery_point(&chain.backup, parse_fault_time(fault_at)?)?;
    for database in array(&plan["databases"], "产品数据库")? {
        exact(
            database,
            &["source_key", "target_key", "server_uuid", "database"],
            "产品数据库",
        )?;
    }
    let expected = project(chain, id, fault_at)?;
    for field in product_fields() {
        if plan[field] != expected[field] {
            return Err(
                format!("产品恢复计划字段 {field} 与登记目标、备份或比较来源不一致").into(),
            );
        }
    }
    Ok(())
}

fn project(chain: &Chain, id: &str, fault_at: &str) -> Result<Value> {
    project_inputs(
        &chain.reference.value,
        &chain.arm.target.selected["worker_ready_url"],
        &chain::frontend_sha(chain)?,
        id,
        fault_at,
    )
}

fn project_inputs(
    reference: &Value,
    worker_ready: &Value,
    frontend_sha: &str,
    id: &str,
    fault_at: &str,
) -> Result<Value> {
    let target = &reference["target"];
    let scope = text(&target["scope_id"], "参考目标 scope")?;
    let api = text(&target["api_url"], "参考目标 API")?;
    let api_ready = json!(format!("{}/readyz", api.trim_end_matches('/')));
    canonical_endpoint(&api_ready, "/readyz", "恢复 API 探针")?;
    canonical_endpoint(worker_ready, "/readyz", "恢复 Worker 探针")?;
    if &api_ready == worker_ready {
        return Err("恢复 API 和 Worker 探针不得重叠".into());
    }
    let databases = array(&target["databases"], "参考目标数据库")?
        .iter()
        .map(|database| {
            json!({
                "source_key":database["key"],
                "target_key":database["key"],
                "server_uuid":database["server_uuid"],
                "database":database["database"],
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "id":id,
        "backup_id":reference["id"],
        "scope_id":scope,
        "fault_at":fault_at,
        "databases":databases,
        "object_endpoint":target["s3"]["endpoint"],
        "object_prefix":format!("{scope}/"),
        "api_ready_url":api_ready,
        "worker_ready_url":worker_ready,
        "frontend_sha":frontend_sha,
    }))
}

fn parse_fault_time(value: &str) -> Result<DateTime<Utc>> {
    if !value.ends_with('Z') {
        return Err("恢复故障时间必须使用 UTC Z 格式".into());
    }
    let fault = value
        .parse::<DateTime<Utc>>()
        .map_err(|_| "恢复故障时间不是有效 UTC 时间")?;
    if fault.nanosecond() % 1_000 != 0
        || fault.to_rfc3339_opts(SecondsFormat::AutoSi, true) != value
    {
        return Err("产品恢复故障时间必须使用 Rust 原生 UTC Z 格式及秒、毫秒或微秒精度".into());
    }
    Ok(fault)
}

fn validate_recovery_point(backup: &Backup, fault: DateTime<Utc>) -> Result<()> {
    let captured = timestamp(&backup.manifest["captured_at"], "备份采集时间")?.with_timezone(&Utc);
    if fault < captured || fault > Utc::now() || fault - captured > Duration::hours(24) {
        return Err("恢复故障时间必须位于实际备份采集后 24 小时内且不能在未来".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery_inputs_fixture::Fixture;

    #[test]
    fn independent_b1_product_rejects_a_rebound_export_with_identical_content() -> Result<()> {
        let fixture = Fixture::new();
        let comparison = document::read(&fixture.comparison_sources, &fixture.backend, "比较来源")?;
        let comparison = comparison::validate(&fixture.backend, &comparison)?;
        let original = &comparison["source_export"];
        let mut replacement = original.clone();
        let path = fixture.root.join("rebound-export.json");
        std::fs::copy(text(&original["result"]["path"], "导出结果")?, &path)?;
        replacement["result"]["path"] = json!(path);
        replacement
            .as_object_mut()
            .unwrap()
            .remove("identity_sha256");
        replacement["identity_sha256"] = json!(document::hash(&replacement)?);

        require_same_export(original, original)?;
        assert_eq!(
            original["result"]["sha256"],
            replacement["result"]["sha256"]
        );
        assert!(require_same_export(original, &replacement).is_err());
        Ok(())
    }
}
