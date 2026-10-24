use std::collections::BTreeMap;

use chrono::NaiveDateTime;
use regex::Regex;
use serde_json::{Value, json};

use super::{
    digest,
    json::{array, exact, text},
    sql::{Row, SqlValue},
};
use crate::Result;

const DATE_COLUMNS: &[&str] = &["next_run_at", "last_run_at", "created_at", "updated_at"];
pub(crate) const TRIGGERS: &[&str] = &["scheduled", "misfire", "manual"];

type ActionKey = (String, String);

pub(crate) fn request(args: &Value) -> Result<Value> {
    exact(
        args,
        &["copy_stage", "target_schedule_actions"],
        "复制调度待办参数",
    )?;
    actions(args)?;
    // 规则校验使用索引，协议返回保留调用方的声明顺序。
    Ok(args["target_schedule_actions"].clone())
}

pub(crate) fn actions(value: &Value) -> Result<BTreeMap<ActionKey, Value>> {
    let stage = text(
        value.get("copy_stage").ok_or("复制输入缺少 copy_stage")?,
        "copy_stage",
    )?;
    let items = array(
        value
            .get("target_schedule_actions")
            .ok_or("复制输入缺少 target_schedule_actions")?,
        "target_schedule_actions",
    )?;
    if !matches!(stage, "source_to_seed" | "seed_to_arm") || items.len() > 100 {
        return Err("复制阶段或目标调度处置清单无效".into());
    }
    if !items.is_empty() && stage != "source_to_seed" {
        return Err("只有来源到 seed 可以声明目标调度待办；两侧必须复制已完成处置的 seed".into());
    }
    let schedule = Regex::new(r"^[1-9]\d{0,18}$")?;
    let handler = Regex::new(r"^[a-z][a-z0-9_.-]{0,95}$")?;
    let sha = Regex::new(r"^[a-f0-9]{64}$")?;
    let mut result = BTreeMap::new();
    for item in items {
        exact(
            item,
            &[
                "action",
                "tenant_id",
                "schedule_id",
                "handler_key",
                "expected_version",
                "source_row_sha256",
            ],
            "目标调度动作",
        )?;
        let object = item.as_object().unwrap();
        let action = text(object.get("action").unwrap(), "action")?;
        let tenant = text(object.get("tenant_id").unwrap(), "tenant_id")?;
        let schedule_id = text(object.get("schedule_id").unwrap(), "schedule_id")?;
        let handler_key = text(object.get("handler_key").unwrap(), "handler_key")?;
        let expected_version = object
            .get("expected_version")
            .and_then(Value::as_i64)
            .filter(|value| (1..i64::MAX).contains(value))
            .ok_or("目标调度动作版本无效")?;
        let source_sha = text(
            object.get("source_row_sha256").unwrap(),
            "source_row_sha256",
        )?;
        let parsed_id = schedule_id.parse::<i64>().ok();
        if action != "disable_schedule_via_api"
            || tenant != "system"
            || !schedule.is_match(schedule_id)
            || parsed_id.is_none_or(|value| value <= 0)
            || !handler.is_match(handler_key)
            || !sha.is_match(source_sha)
        {
            return Err("仅接受明确 system 记录、版本、处理器与完整源行摘要的停用待办".into());
        }
        let key = (tenant.to_owned(), schedule_id.to_owned());
        if result.insert(key, item.clone()).is_some() {
            return Err("同一目标调度待办重复".into());
        }
        let _ = expected_version;
    }
    Ok(result)
}

pub(crate) fn inspect(
    row: &Row,
    actions: &mut BTreeMap<ActionKey, Value>,
) -> Result<Option<Value>> {
    let enabled = required_integer(row, "enabled")?;
    let deleted = required_text(row, "del_flag")?;
    if !matches!(enabled, 0 | 1) || !matches!(deleted, "0" | "2") {
        return Err("来源调度状态无效".into());
    }
    if deleted == "2" || enabled == 0 {
        return Ok(None);
    }
    let tenant = required_text(row, "tenant_id")?;
    let id = required_text_number(row, "id")?;
    let key = (tenant.to_owned(), id.clone());
    let action = actions
        .remove(&key)
        .ok_or("启用调度没有精确匹配的目标处置清单、版本或完整源行摘要")?;
    let object = action.as_object().unwrap();
    let expected_version = object
        .get("expected_version")
        .and_then(Value::as_i64)
        .unwrap();
    if object.get("handler_key").and_then(Value::as_str) != Some(required_text(row, "handler_key")?)
        || expected_version != required_integer(row, "version")?
        || object.get("source_row_sha256").and_then(Value::as_str)
            != Some(row_sha256(row)?.as_str())
    {
        return Err("启用调度没有精确匹配的目标处置清单、版本或完整源行摘要".into());
    }
    let mut pending = object.clone();
    pending.insert(
        "operations".into(),
        json!([
            "get_monitor_schedules_by_id",
            "put_monitor_schedules_by_id_status"
        ]),
    );
    pending.insert("path".into(), json!({"id": id}));
    pending.insert(
        "body".into(),
        json!({"version": expected_version, "enabled": false}),
    );
    pending.insert(
        "required_permissions".into(),
        json!(["monitor:schedule:list", "monitor:schedule:edit"]),
    );
    pending.insert("worker_must_remain_stopped".into(), Value::Bool(true));
    pending.insert(
        "required_api_jobs_mode".into(),
        Value::String("external".into()),
    );
    pending.insert("api_called".into(), Value::Bool(false));
    pending.insert(
        "completion_evidence".into(),
        Value::String(
            "目标作用域及原行摘要核验、GET/PUT版本响应、enabled=false/next_run_at=NULL/version+1、源未变；未知结果先核对，不重放"
                .into(),
        ),
    );
    Ok(Some(Value::Object(pending)))
}

pub(crate) fn row_sha256(row: &Row) -> Result<String> {
    let date = Regex::new(r"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(?:\.\d{1,6})?$")?;
    let mut entries = Vec::with_capacity(row.len());
    for (name, value) in row {
        let canonical = if DATE_COLUMNS.contains(&name.as_str()) {
            match value {
                SqlValue::Null => json!(["null"]),
                SqlValue::Text(value) => {
                    if !date.is_match(value) {
                        return Err("完整调度行的 UTC 日期格式无效".into());
                    }
                    let parsed = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
                        .map_err(|_| "完整调度行的日期类型无效")?;
                    json!([
                        "datetime-utc",
                        parsed.format("%Y-%m-%d %H:%M:%S%.6f").to_string()
                    ])
                }
                _ => return Err("完整调度行的日期类型无效".into()),
            }
        } else {
            canonical_sql(value)?
        };
        entries.push(Value::Array(vec![Value::String(name.clone()), canonical]));
    }
    digest::plan_hash(&json!({
        "kind": "devex-schedule-source-row-v1",
        "row": ["object", entries],
    }))
}

fn canonical_sql(value: &SqlValue) -> Result<Value> {
    Ok(match value {
        SqlValue::Null => json!(["null"]),
        SqlValue::Bytes(value) => json!(["bytes", hex::encode(value)]),
        SqlValue::Number(value) => json!(["decimal", normalize_decimal(value)?]),
        SqlValue::Text(value) => json!(["text", value]),
        SqlValue::Json(value) => canonical_json(value)?,
    })
}

fn canonical_json(value: &Value) -> Result<Value> {
    Ok(match value {
        Value::Null => json!(["null"]),
        Value::Bool(value) => json!(["boolean", value]),
        Value::Number(value) => json!(["decimal", normalize_decimal(value.as_str())?]),
        Value::String(value) => json!(["text", value]),
        Value::Array(values) => {
            let values = values
                .iter()
                .map(canonical_json)
                .collect::<Result<Vec<_>>>()?;
            json!(["array", values])
        }
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort();
            let entries = keys
                .into_iter()
                .map(|key| Ok(json!([key, canonical_json(&values[key])?])))
                .collect::<Result<Vec<_>>>()?;
            json!(["object", entries])
        }
    })
}

pub(crate) fn normalize_decimal(value: &str) -> Result<String> {
    let (negative, unsigned) = value
        .strip_prefix('-')
        .map_or((false, value), |value| (true, value));
    let unsigned = unsigned.strip_prefix('+').unwrap_or(unsigned);
    let exponent_at = unsigned.find(['e', 'E']);
    let (mantissa, exponent) = exponent_at.map_or((unsigned, 0_i64), |index| {
        (
            &unsigned[..index],
            unsigned[index + 1..].parse::<i64>().unwrap_or(i64::MAX),
        )
    });
    if exponent.unsigned_abs() > 100_000 {
        return Err("完整行摘要包含过大的十进制指数".into());
    }
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if integer
        .chars()
        .chain(fraction.chars())
        .any(|character| !character.is_ascii_digit())
    {
        return Err("完整行摘要包含无效十进制数".into());
    }
    let digits = format!("{integer}{fraction}");
    let scale = fraction.len() as i64 - exponent;
    let mut text = if scale <= 0 {
        format!("{digits}{}", "0".repeat((-scale) as usize))
    } else {
        let scale = scale as usize;
        if digits.len() <= scale {
            format!("0.{}{digits}", "0".repeat(scale - digits.len()))
        } else {
            format!(
                "{}.{}",
                &digits[..digits.len() - scale],
                &digits[digits.len() - scale..]
            )
        }
    };
    let (integer, fraction) = text
        .split_once('.')
        .map_or((text.as_str(), None), |(integer, fraction)| {
            (integer, Some(fraction))
        });
    let integer = integer.trim_start_matches('0');
    let integer = if integer.is_empty() { "0" } else { integer };
    text = match fraction {
        Some(fraction) => {
            let fraction = fraction.trim_end_matches('0');
            if fraction.is_empty() {
                integer.to_owned()
            } else {
                format!("{integer}.{fraction}")
            }
        }
        None => integer.to_owned(),
    };
    if negative && text != "0" {
        text.insert(0, '-');
    }
    Ok(text)
}

fn required_text<'a>(row: &'a Row, name: &str) -> Result<&'a str> {
    row.get(name)
        .and_then(SqlValue::text)
        .ok_or_else(|| format!("调度行 {name} 必须为字符串").into())
}

fn required_integer(row: &Row, name: &str) -> Result<i64> {
    row.get(name)
        .and_then(SqlValue::integer)
        .ok_or_else(|| format!("调度行 {name} 必须为整数").into())
}

fn required_text_number(row: &Row, name: &str) -> Result<String> {
    let value = row.get(name).ok_or_else(|| format!("调度行缺少 {name}"))?;
    match value {
        SqlValue::Text(value) => Ok(value.clone()),
        SqlValue::Number(value) => Ok(value.clone()),
        _ => Err(format!("调度行 {name} 必须为标识").into()),
    }
}
