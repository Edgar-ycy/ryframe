use std::path::Path;

use chrono::{DateTime, Duration, SecondsFormat, Timelike, Utc};
use serde_json::{Value, json};

use super::{
    chain::{self, Chain},
    document::{array, exact, text},
    domain::{canonical_endpoint, identifier, product_fields, scope_identifier, timestamp},
};
use crate::{Result, cli::ProductInputOptions};

pub(crate) fn build(backend: &Path, options: &ProductInputOptions) -> Result<Value> {
    identifier(&options.id)?;
    let fault = parse_fault_time(&options.fault_at)?;
    let chain = chain::load(
        backend,
        &options.reference_plan,
        &options.backup_receipt,
        &options.comparison_sources,
        &options.arm_input,
        &options.fresh_target_verify,
        options.side.as_str(),
    )?;
    validate_recovery_point(&chain, fault)?;
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

pub(super) fn validate_product(chain: &Chain, value: &Value) -> Result<()> {
    let plan = exact(value, &product_fields(), "产品恢复计划")?;
    let id = text(&plan["id"], "产品恢复 ID")?;
    identifier(id)?;
    identifier(text(&plan["backup_id"], "产品备份 ID")?)?;
    scope_identifier(text(&plan["scope_id"], "产品 scope")?)?;
    let fault_at = text(&plan["fault_at"], "产品故障时间")?;
    validate_recovery_point(chain, parse_fault_time(fault_at)?)?;
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
    let reference = &chain.reference.value;
    let target = &reference["target"];
    let scope = text(&target["scope_id"], "参考目标 scope")?;
    let api = text(&target["api_url"], "参考目标 API")?;
    let api_ready = json!(format!("{}/readyz", api.trim_end_matches('/')));
    let worker_ready = &chain.arm.target.selected["worker_ready_url"];
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
        "frontend_sha":chain::frontend_sha(chain)?,
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

fn validate_recovery_point(chain: &Chain, fault: DateTime<Utc>) -> Result<()> {
    let captured =
        timestamp(&chain.backup.manifest["captured_at"], "备份采集时间")?.with_timezone(&Utc);
    if fault < captured || fault > Utc::now() || fault - captured > Duration::hours(24) {
        return Err("恢复故障时间必须位于实际备份采集后 24 小时内且不能在未来".into());
    }
    Ok(())
}
