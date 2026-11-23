use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use super::{exact, inputs, native, text};
use crate::Result;
pub(super) use crate::recovery::inputs::product_record::product_hash;

pub(in crate::recovery) fn backup_source(args: &Value) -> Result<Value> {
    exact(
        args,
        &[
            "plan",
            "inventory",
            "identity",
            "source",
            "exported",
            "build",
            "descriptor",
            "runtime",
            "quiescence",
        ],
    )?;
    let identity = &args["identity"];
    let source = &args["source"];
    let exported = &args["exported"];
    let generation = &exported["generation"];
    let request = &exported["request"];
    let mut expected = native::json::object(&exported["inventory"], "导出库存")?.clone();
    // 原始库存不得覆盖正式计划ID。
    if expected.contains_key("id") {
        return Err("共享导出原始库存不接受备份ID".into());
    }
    expected.insert("id".into(), args["plan"]["id"].clone());
    if identity["source_generation"] != args["descriptor"]
        || source["source_generation"] != args["descriptor"]
        || args["inventory"] != Value::Object(expected)
        || request["source"] != args["plan"]["source"]
        || request["tools"] != args["plan"]["tools"]
        || *request != source["request"]
        || *generation != source["generation"]
        || generation["source"] != args["build"]["sources"]["full"]["source"]["snapshot"]
    {
        return Err("正式备份与共享导出的库存、配置、构建或运行代次不同".into());
    }
    let times = [
        &args["runtime"]["verified_at"],
        &args["quiescence"]["observed_stopped_at"],
        &args["inventory"]["quiesced_at"],
        &args["inventory"]["captured_at"],
    ]
    .into_iter()
    .map(|value| DateTime::parse_from_rfc3339(text(value)?).map_err(Into::into))
    .collect::<Result<Vec<_>>>()?;
    if times.windows(2).any(|pair| pair[0] > pair[1]) || times[3] > Utc::now() {
        return Err("同代源验证、实际停止观察和正式库存采集时间顺序不成立".into());
    }
    Ok(json!({"source_export":identity,"source_generation":args["descriptor"]}))
}

pub(super) fn backup_result(args: &Value) -> Result<Value> {
    exact(args, &["value"])?;
    let value = &args["value"];
    exact(
        value,
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
    )?;
    if value["format_version"].as_u64() != Some(2)
        || value["kind"] != "restore-reference-backup"
        || value["artifacts"].as_u64().is_none_or(|count| count == 0)
    {
        return Err("正式备份结果版本、类型或产物数量无效".into());
    }
    Ok(Value::Bool(true))
}

pub(super) fn record(args: &Value) -> Result<Value> {
    exact(args, &["plan", "manifest", "record", "product_plan"])?;
    let record = &args["record"];
    exact(
        record,
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
    )?;
    let product = &record["plan"];
    if *product != args["product_plan"] || record["plan_hash"] != product_hash(product)? {
        return Err("restore-begin记录与目标计划内嵌产品计划或产品摘要不完全一致".into());
    }
    for field in ["id", "backup_id"] {
        inputs::domain::identifier(text(&product[field])?)?;
    }
    inputs::domain::scope_identifier(text(&product["scope_id"])?)?;
    let target = &args["plan"]["target"];
    let expected = native::json::array(&target["databases"], "目标数据库")?
        .iter()
        .map(|database| {
            Ok((
                text(&database["key"])?,
                text(&database["key"])?,
                text(&database["server_uuid"])?,
                text(&database["database"])?,
            ))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let actual_values = native::json::array(&product["databases"], "产品数据库")?;
    let actual = actual_values
        .iter()
        .map(|database| {
            Ok((
                text(&database["source_key"])?,
                text(&database["target_key"])?,
                text(&database["server_uuid"])?,
                text(&database["database"])?,
            ))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    if record["status"] != "running"
        || product["backup_id"] != args["manifest"]["id"]
        || product["backup_id"] != args["plan"]["id"]
        || product["scope_id"] != target["scope_id"]
        || actual != expected
        || actual_values.len() != expected.len()
        || product["object_endpoint"] != target["s3"]["endpoint"]
        || product["object_prefix"] != format!("{}/", text(&target["scope_id"])?)
        || ["data_verified_at", "completed_at", "failure"]
            .into_iter()
            .any(|field| !record[field].is_null())
    {
        return Err("还原必须绑定restore-begin的运行中记录和相同精确目标".into());
    }
    let started = DateTime::parse_from_rfc3339(text(&record["started_at"])?)?;
    let recovered = DateTime::parse_from_rfc3339(text(&record["recovered_at"])?)?;
    let captured = DateTime::parse_from_rfc3339(text(&args["manifest"]["captured_at"])?)?;
    if recovered != captured || recovered > started {
        return Err("恢复记录的恢复点与同一备份清单不一致".into());
    }
    let elapsed = Utc::now().signed_duration_since(started);
    if elapsed < chrono::Duration::zero() || elapsed > chrono::Duration::hours(1) {
        return Err("恢复操作尚未开始、时钟回退或已超过60分钟".into());
    }
    Ok(Value::Bool(true))
}
