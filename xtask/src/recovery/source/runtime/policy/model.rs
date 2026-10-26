//! 登录原始行、缓存主体与远端写入计数；未知结果只记录已知事实。
use super::documents;
use crate::Result;
use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const LOGIN_FIELDS: &[&str] = &[
    "id",
    "tenant_id",
    "user_name",
    "ipaddr",
    "login_location",
    "browser",
    "os",
    "status",
    "msg",
    "login_time",
];

pub(super) fn login_rows(args: &Value) -> Result<Value> {
    documents::exact(args, &["raw"], "来源登录原始行参数")?;
    let raw = args["raw"].as_str().ok_or("来源登录日志不是文本")?;
    let id = Regex::new(r"^[1-9][0-9]{0,18}$")?;
    let hex = Regex::new(r"^[A-F0-9]*$")?;
    let mut previous = 0_u64;
    let mut result = Vec::new();
    for line in raw.lines() {
        let columns: Vec<_> = line.split('\t').collect();
        if columns.len() != LOGIN_FIELDS.len() || !id.is_match(columns[0]) {
            return Err("来源登录日志查询返回无效列或主键".into());
        }
        let current = columns[0].parse::<u64>()?;
        if current <= previous {
            return Err("来源登录日志查询没有按唯一主键排序".into());
        }
        previous = current;
        let mut row = serde_json::Map::new();
        for (index, (field, text)) in LOGIN_FIELDS.iter().zip(columns).enumerate() {
            let value = if index == 0 || index == 9 {
                json!(text)
            } else if [4, 5, 6, 8].contains(&index) && text == "NULL" {
                Value::Null
            } else {
                if text.len() % 2 != 0 || !hex.is_match(text) {
                    return Err("来源登录日志包含无效十六进制文本".into());
                }
                let bytes = hex::decode(text).map_err(|_| "来源登录日志包含无效十六进制文本")?;
                json!(String::from_utf8(bytes).map_err(|_| "来源登录日志文本不是有效 UTF-8")?)
            };
            row.insert((*field).into(), value);
        }
        result.push(Value::Object(row));
    }
    Ok(json!(result))
}

pub(super) fn snapshot(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["payload", "subject", "tenant_epoch"],
        "来源授权快照参数",
    )?;
    let payload = args["payload"].as_str().ok_or("来源授权快照不是文本")?;
    let value = documents::parse_fragment(payload, "来源授权快照")
        .map_err(|_| "来源授权快照不是有效且字段唯一的 JSON")?;
    documents::exact(
        &value,
        &["versions", "tenant_session_version", "principal"],
        "来源授权快照",
    )?;
    let versions = &value["versions"];
    documents::exact(
        versions,
        &["tenant_authorization_epoch", "user_authorization_version"],
        "来源授权快照版本",
    )?;
    let principal = &value["principal"];
    documents::exact(
        principal,
        &[
            "actor",
            "tenant_authorization_epoch",
            "preferred_locale",
            "roles",
            "role_ids",
            "permissions",
            "tenant_request_limit_per_minute",
        ],
        "来源授权快照主体",
    )?;
    let actor = &principal["actor"];
    documents::exact(
        actor,
        &[
            "user_id",
            "tenant_id",
            "username",
            "dept_id",
            "dept_path",
            "data_scope",
            "custom_dept_ids",
            "include_self",
            "is_super_admin",
        ],
        "来源授权快照 actor",
    )?;
    let epoch = documents::text(&args["tenant_epoch"], "来源授权快照 epoch")?
        .parse::<u64>()
        .map_err(|_| "来源授权快照 epoch 无效")?;
    let subject = &args["subject"];
    let actor_id = actor["user_id"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| actor["user_id"].as_u64().map(|id| id.to_string()));
    if actor_id.as_deref() != subject["user_id"].as_str()
        || actor["tenant_id"] != subject["tenant_id"]
        || versions["tenant_authorization_epoch"].as_u64() != Some(epoch)
        || versions["user_authorization_version"] != subject["user_authorization_version"]
        || principal["tenant_authorization_epoch"].as_u64() != Some(epoch)
    {
        return Err("来源授权快照与登录主体或版本不同".into());
    }
    Ok(json!({"bytes":payload.len(),"sha256":hex::encode(Sha256::digest(payload.as_bytes()))}))
}

pub(super) fn remote_writes(args: &Value) -> Result<Value> {
    documents::exact(args, &["cleanup", "recovered"], "来源远端写入计数参数")?;
    let cleanup = &args["cleanup"];
    let recovered = args["recovered"].as_bool().ok_or("来源恢复标志无效")?;
    let expired = documents::array(&cleanup["expired_snapshot_keys"], "来源过期快照")?.len();
    let after = documents::array(&cleanup["after"], "来源缓存后像")?.len();
    let reconciled = match cleanup["execution"].as_str() {
        Some("confirmed_watch_exec") => false,
        Some("reconciled_unknown_exec_result") if recovered && cleanup["deleted"].is_null() => true,
        _ => return Err("来源授权缓存执行类型无效，未知写入不能推定成功或重放".into()),
    };
    if !reconciled
        && cleanup["deleted"]
            .as_u64()
            .is_none_or(|n| n + expired as u64 != after as u64)
    {
        return Err("来源授权缓存计数不能解释完整后像".into());
    }
    Ok(json!({"api_session_commands":22,"authorization_cache":{
        "confirmed_commands":u8::from(!reconciled),
        "unknown_result_commands":if reconciled {Value::Null} else {json!(0)},
        "phase":if recovered {"recovery"} else {"initial_verification"},
        "historical_cleanup":if recovered {super::cache_attempt::historical(cleanup)?} else {"not_applicable"},
        "deleted_keys":cleanup["deleted"],"expired_snapshot_keys":expired,"observed_absent_keys":after},
        "confirmed_total_commands":22+u8::from(!reconciled)}))
}

pub(super) fn time_order(args: &Value) -> Result<Value> {
    documents::exact(args, &["values", "label"], "来源时间序列参数")?;
    // 标签来自调用方，诊断不回显以避免任意输入携带凭据。
    documents::text(&args["label"], "来源时间序列标签")?;
    let values = documents::array(&args["values"], "来源时间序列")?;
    if values.is_empty() {
        return Err("来源时间序列为空".into());
    }
    let mut previous = None;
    for value in values {
        let time = DateTime::parse_from_rfc3339(documents::text(value, "来源时间")?)
            .map_err(|_| "来源时间无效")?
            .with_timezone(&Utc);
        if time > Utc::now() || previous.is_some_and(|previous| previous > time) {
            return Err("来源运行收据时间顺序不成立".into());
        }
        previous = Some(time);
    }
    Ok(Value::Null)
}
