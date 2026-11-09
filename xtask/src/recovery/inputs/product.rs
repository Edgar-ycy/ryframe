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
    validate_fault_shape(&options.fault_at)?;
    let chain = chain::load(
        backend,
        &options.reference_plan,
        &options.backup_receipt,
        &options.comparison_sources,
        &options.arm_input,
        &options.fresh_target_verify,
        options.side.as_str(),
    )?;
    let target = chain
        .reference
        .value
        .get("target")
        .ok_or("参考计划缺少目标")?;
    let databases = array(
        target.get("databases").ok_or("参考目标缺少数据库")?,
        "参考目标数据库",
    )?
    .iter()
    .map(|database| {
        let database = exact(
            database,
            &[
                "key",
                "kind",
                "mode",
                "database",
                "server_uuid",
                "defaults_file",
                "defaults_sha256",
            ],
            "参考目标数据库",
        )?;
        Ok(json!({
            "source_key": database.get("key").unwrap(),
            "target_key": database.get("key").unwrap(),
            "server_uuid": database.get("server_uuid").unwrap(),
            "database": database.get("database").unwrap(),
        }))
    })
    .collect::<Result<Vec<_>>>()?;
    let scope = text(
        target.get("scope_id").ok_or("参考目标缺少 scope")?,
        "参考目标 scope",
    )?;
    let api = text(
        target.get("api_url").ok_or("参考目标缺少 API")?,
        "参考目标 API",
    )?;
    let value = json!({
        "id": options.id,
        "backup_id": chain.reference.value.get("id").unwrap(),
        "scope_id": scope,
        "fault_at": options.fault_at,
        "databases": databases,
        "object_endpoint": target.get("s3").and_then(|value| value.get("endpoint")).unwrap(),
        "object_prefix": format!("{scope}/"),
        "api_ready_url": format!("{}/readyz", api.trim_end_matches('/')),
        "worker_ready_url": target.get("worker_ready_url").unwrap(),
        "frontend_sha": chain::frontend_sha(&chain)?,
    });
    validate_product(&chain, &value)?;
    chain.reference.assert_unchanged()?;
    chain.backup_document.assert_unchanged()?;
    chain.comparison_document.assert_unchanged()?;
    chain.arm.result.assert_unchanged()?;
    chain.arm.request.assert_unchanged()?;
    chain.arm.manifest.assert_unchanged()?;
    chain.arm.source_request.assert_unchanged()?;
    Ok(value)
}

pub(crate) fn validate_product(chain: &Chain, value: &Value) -> Result<()> {
    let value_object = exact(value, &product_fields(), "产品恢复计划")?;
    let id = text(value_object.get("id").unwrap(), "产品恢复 ID")?;
    identifier(id)?;
    identifier(text(value_object.get("backup_id").unwrap(), "产品备份 ID")?)?;
    scope_identifier(text(value_object.get("scope_id").unwrap(), "产品 scope")?)?;
    let fault_at = text(value_object.get("fault_at").unwrap(), "产品故障时间")?;
    validate_fault_shape(fault_at)?;
    validate_fault_time(
        fault_at,
        chain
            .backup
            .manifest
            .get("captured_at")
            .ok_or("备份清单缺少采集时间")?,
    )?;
    let target = chain
        .reference
        .value
        .get("target")
        .ok_or("参考计划缺少目标")?;
    let expected_databases = array(
        target.get("databases").ok_or("参考目标缺少数据库")?,
        "数据库",
    )?
    .iter()
    .map(|database| {
        json!({
            "source_key": database["key"],
            "target_key": database["key"],
            "server_uuid": database["server_uuid"],
            "database": database["database"],
        })
    })
    .collect::<Vec<_>>();
    for database in array(value_object.get("databases").unwrap(), "产品数据库")? {
        exact(
            database,
            &["source_key", "target_key", "server_uuid", "database"],
            "产品数据库",
        )?;
    }
    let selected = &chain.arm.target.selected;
    let frontend_sha = chain::frontend_sha(chain)?;
    if value_object.get("backup_id") != chain.reference.value.get("id")
        || value_object.get("scope_id") != target.get("scope_id")
        || value_object.get("databases").and_then(Value::as_array) != Some(&expected_databases)
        || value_object.get("object_endpoint")
            != target.get("s3").and_then(|value| value.get("endpoint"))
        || value_object.get("object_prefix")
            != Some(&Value::String(format!(
                "{}/",
                text(target.get("scope_id").unwrap(), "scope")?
            )))
        || value_object.get("frontend_sha") != Some(&Value::String(frontend_sha))
        || value_object.get("api_ready_url")
            != Some(&Value::String(format!(
                "{}/readyz",
                text(target.get("api_url").unwrap(), "API")?.trim_end_matches('/')
            )))
        || value_object.get("worker_ready_url") != selected.get("worker_ready_url")
    {
        return Err("产品 restore plan 未绑定精确目标、探针或对应 arm 前端 SHA".into());
    }
    for field in ["api_ready_url", "worker_ready_url"] {
        canonical_endpoint(value_object.get(field).unwrap(), "/readyz", "恢复探针")?;
    }
    if value_object.get("api_ready_url") == value_object.get("worker_ready_url") {
        return Err("恢复 API 和 Worker 探针不得重叠".into());
    }
    Ok(())
}

fn validate_fault_shape(value: &str) -> Result<()> {
    let Some(body) = value.strip_suffix('Z') else {
        return Err("恢复故障时间必须使用 UTC Z 格式".into());
    };
    let precision = body
        .split_once('.')
        .map(|(_, fraction)| fraction.len())
        .unwrap_or(0);
    if !matches!(precision, 0 | 3 | 6)
        || DateTime::parse_from_rfc3339(value).is_err()
        || body
            .split_once('.')
            .is_some_and(|(_, fraction)| !fraction.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err("恢复故障时间必须使用秒、毫秒或微秒精度".into());
    }
    Ok(())
}

fn validate_fault_time(value: &str, captured: &Value) -> Result<()> {
    let fault = timestamp(&Value::String(value.to_owned()), "恢复故障时间")?.with_timezone(&Utc);
    let captured = timestamp(captured, "备份采集时间")?.with_timezone(&Utc);
    let now = Utc::now();
    if fault < captured || fault > now || fault - captured > Duration::hours(24) {
        return Err("恢复故障时间必须位于实际备份采集后 24 小时内且不能在未来".into());
    }
    let precision = if fault.nanosecond() == 0 {
        SecondsFormat::Secs
    } else if fault.nanosecond() % 1_000_000 == 0 {
        SecondsFormat::Millis
    } else {
        SecondsFormat::Micros
    };
    let canonical = fault.to_rfc3339_opts(precision, true);
    if canonical != value {
        return Err("产品恢复故障时间必须使用 Rust 原生 UTC Z 格式及秒、毫秒或微秒精度".into());
    }
    Ok(())
}
