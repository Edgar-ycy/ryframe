//! 授权缓存收据与未知结果恢复的纯模型；未知结果不能伪造删除数或触发重放。
use std::collections::BTreeSet;

use regex::Regex;
use serde_json::{Value, json};

use super::{documents, model};
use crate::Result;

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/source/runtime/cache/tests.rs"]
mod tests;

pub(super) fn keys(plan: &Value) -> Result<Vec<Value>> {
    let mut keys = Vec::new();
    for row in documents::array(plan, "授权缓存计划")? {
        for role in ["tenant_epoch", "user_version", "snapshot"] {
            documents::text(&row["keys"][role], "授权缓存键")?;
            keys.push(row["keys"][role].clone());
        }
    }
    Ok(keys)
}

fn validate_before(before: &Value, plan: &Value, expired: &Value) -> Result<()> {
    let before = documents::array(before, "授权缓存前像")?;
    let plan = documents::array(plan, "授权缓存计划")?;
    let expired = documents::array(expired, "授权缓存过期快照")?;
    if before.len() != plan.len() {
        return Err("来源授权缓存前像没有完整主体集合".into());
    }
    let epoch = Regex::new(r"^[1-9][0-9]{0,9}$")?;
    let digest = Regex::new(r"^[a-f0-9]{64}$")?;
    for (actual, expected) in before.iter().zip(plan) {
        documents::exact(
            actual,
            &[
                "tenant_id",
                "user_id",
                "user_authorization_version",
                "tenant_epoch",
                "user_version",
                "snapshot",
            ],
            "授权缓存清理前像",
        )?;
        for key in ["tenant_id", "user_id", "user_authorization_version"] {
            if actual[key] != expected[key] {
                return Err("来源授权缓存清理主体不同".into());
            }
        }
        let tenant = &actual["tenant_epoch"];
        let version = &actual["user_version"];
        let snapshot = &actual["snapshot"];
        documents::exact(tenant, &["key", "value"], "授权缓存tenant epoch")?;
        documents::exact(version, &["key", "value"], "授权缓存用户版本")?;
        let fields = match snapshot["type"].as_str() {
            Some("hash") => &["key", "type", "field", "bytes", "sha256"][..],
            Some("none") => &["key", "type"][..],
            _ => return Err("来源授权缓存快照类型不同".into()),
        };
        documents::exact(snapshot, fields, "授权缓存快照")?;
        let epoch_value = documents::text(&tenant["value"], "授权缓存tenant epoch")?;
        let expected_version = expected["user_authorization_version"]
            .as_u64()
            .ok_or("授权版本无效")?
            .to_string();
        if tenant["key"] != expected["keys"]["tenant_epoch"]
            || !epoch.is_match(epoch_value)
            || version != &json!({"key":expected["keys"]["user_version"],"value":expected_version})
            || snapshot["key"] != expected["keys"]["snapshot"]
        {
            return Err("来源授权缓存清理键或版本不同".into());
        }
        if snapshot["type"] == "none" {
            if !expired.contains(&snapshot["key"]) {
                return Err("来源授权缓存过期快照没有计入原子清理".into());
            }
        } else if expired.contains(&snapshot["key"])
            || snapshot["field"] != format!("{epoch_value}:{expected_version}")
            || snapshot["bytes"].as_u64().is_none_or(|n| n == 0)
            || snapshot["sha256"]
                .as_str()
                .is_none_or(|s| !digest.is_match(s))
        {
            return Err("来源授权缓存清理键或版本不同".into());
        }
    }
    Ok(())
}

pub(super) fn observation(args: &Value) -> Result<Value> {
    documents::exact(args, &["value", "subjects"], "恢复授权缓存前像参数")?;
    validate_observation(&args["value"], &args["subjects"])?;
    Ok(args["value"].clone())
}

fn validate_observation(cache: &Value, subjects: &Value) -> Result<()> {
    documents::exact(
        cache,
        &["namespace", "subjects", "plan", "before"],
        "恢复授权缓存前像",
    )?;
    let plan = model::cache_plan(&json!({"namespace":cache["namespace"],"subjects":subjects}))?;
    if cache["subjects"] != *subjects || cache["plan"] != plan {
        return Err("来源验收恢复 intent 的授权缓存计划不同".into());
    }
    let expired = expired(&cache["before"])?;
    validate_before(&cache["before"], &plan, &expired)
}

fn expired(before: &Value) -> Result<Value> {
    let rows = documents::array(before, "授权缓存前像")?;
    Ok(Value::Array(
        rows.iter()
            .filter(|row| row["snapshot"]["type"] == "none")
            .map(|row| row["snapshot"]["key"].clone())
            .collect(),
    ))
}

fn timestamp(value: &Value) -> Result<()> {
    let text = documents::text(value, "授权缓存清理时间")?;
    chrono::DateTime::parse_from_rfc3339(text).map_err(|_| "来源授权缓存清理时间无效")?;
    Ok(())
}

pub(super) fn receipt(args: &Value) -> Result<Value> {
    documents::exact(args, &["value", "subjects"], "授权缓存清理收据参数")?;
    let value = &args["value"];
    documents::exact(
        value,
        &[
            "format_version",
            "kind",
            "status",
            "namespace",
            "before",
            "deleted",
            "expired_snapshot_keys",
            "after",
            "execution",
            "cleaned_at",
        ],
        "授权缓存清理收据",
    )?;
    let plan =
        model::cache_plan(&json!({"namespace":value["namespace"],"subjects":args["subjects"]}))?;
    let expected_keys = keys(&plan)?;
    let snapshots = documents::array(&plan, "授权缓存计划")?
        .iter()
        .map(|row| row["keys"]["snapshot"].clone())
        .collect::<Vec<_>>();
    let expired = documents::array(&value["expired_snapshot_keys"], "授权缓存过期快照")?;
    let unique = expired
        .iter()
        .map(|key| documents::text(key, "过期快照键"))
        .collect::<Result<BTreeSet<_>>>()?;
    let ordered = snapshots
        .into_iter()
        .filter(|key| expired.contains(key))
        .collect::<Vec<_>>();
    let before = documents::array(&value["before"], "授权缓存前像")?;
    let after = documents::array(&value["after"], "授权缓存后像")?;
    let valid_result = match value["execution"].as_str() {
        Some("confirmed_lua_result") => value["deleted"]
            .as_u64()
            .is_some_and(|n| (22..=33).contains(&n) && n + expired.len() as u64 == 33),
        Some("reconciled_unknown_lua_result") => value["deleted"].is_null(),
        _ => false,
    };
    if value["format_version"].as_u64() != Some(2)
        || value["kind"] != "restore-source-authorization-cache-cleanup"
        || value["status"] != "cleaned"
        || !valid_result
        || unique.len() != expired.len()
        || expired != ordered
        || before.len() != 11
        || after.len() != 33
    {
        return Err("来源授权缓存清理身份或结果无效".into());
    }
    for (actual, key) in after.iter().zip(&expected_keys) {
        if actual != &json!({"key":key,"type":"none"}) {
            return Err("来源授权缓存清理后像不同".into());
        }
    }
    validate_before(&value["before"], &plan, &value["expired_snapshot_keys"])?;
    timestamp(&value["cleaned_at"])?;
    Ok(value.clone())
}

pub(super) fn resume(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["frozen", "types", "observed"],
        "授权缓存未知结果恢复参数",
    )?;
    let frozen = &args["frozen"];
    // 所有键已消失也必须先复核冻结计划和前像，不能绕过待续作intent校验。
    validate_observation(frozen, &frozen["subjects"])?;
    let types = documents::array(&args["types"], "授权缓存当前类型")?;
    if types.len() != keys(&frozen["plan"])?.len()
        || types
            .iter()
            .any(|value| value.as_str().is_none_or(str::is_empty))
    {
        return Err("来源授权缓存当前类型集合不完整".into());
    }
    if types.iter().all(|value| value == "none") {
        return Ok(json!({"action":"reconcile","expired":expired(&frozen["before"])?}));
    }
    if args["observed"] != *frozen {
        return Err("来源验收恢复 intent 后授权缓存出现混合状态或漂移".into());
    }
    Ok(json!({"action":"execute"}))
}
