//! 暖来源的原 Redis 状态保持完整，本轮注销只凭同代生产者和 Redis 后像确认。
use super::super::{cache, model, resources, warm_effects};
use crate::{
    Result,
    recovery::clone::native::{digest, json as document, resources::common},
    utc_timestamp::rfc3339_order_key,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

pub(super) fn mode(facts: &Value) -> Result<bool> {
    let registered = resources::warm_redis::applies(&facts["request"]);
    if registered != facts["running"]["image"]["redis"]["existing"].is_object() {
        return Err("来源 Redis 暖基线与已登记的启动请求不符".into());
    }
    Ok(registered)
}

pub(super) fn effects(
    backend: &Path,
    before: &Value,
    after: &Value,
    node: &Value,
    producer: &Value,
    observation: &Value,
) -> Result<Value> {
    let intent = common::read_bound(backend, &producer["intent"], "暖来源生产者启动意图")?;
    let completion = common::read_bound(backend, &producer["completion"], "暖来源生产者完成收据")?;
    let next = warm_effects::compare(
        &before["redis"],
        &after["redis"],
        &node["session_subjects"],
        document::text(&intent["started_at"], "暖来源生产者启动时间")?,
        document::text(&completion["completed_at"], "暖来源生产者完成时间")?,
    )?;

    // Redis 已单独逐项核验，四库及对象继续使用同一份现行副作用模型。
    let mut compared = after.clone();
    compared["redis"] = before["redis"].clone();
    let mut effects = model::write_effects(&json!({"before":before,"after":compared}))?;
    effects["authorization_cache"] = json!({"preserved_keys":observation["preserved_authorization_keys"],
            "expired_snapshot_keys":observation["expired_snapshot_keys"],
            "expired_rate_limit_keys":observation["expired_rate_limit_keys"]});
    effects["refresh_sessions"] = next;
    Ok(effects)
}

pub(super) fn bind_observation(
    observation: &mut Value,
    start: &Value,
    before: &Value,
    node: &Value,
) -> Result<()> {
    observation["source_generation"] = start.clone();
    observation["before"] = before.clone();
    observation["subjects_sha256"] = json!(digest::plan_hash(&node["subjects"])?);
    observation["sessions_sha256"] = json!(digest::plan_hash(&node["session_subjects"])?);
    Ok(())
}

pub(super) fn observation(
    backend: &Path,
    directory: &Path,
    receipt: &Value,
    before: &Value,
    facts: &Value,
    node: &Value,
) -> Result<Value> {
    let descriptor = &receipt["cache_cleanup"];
    if common::bound_file(backend, descriptor, "暖来源短期键观察")?
        != directory.join("cache-cleanup.json")
    {
        return Err("暖来源短期键观察必须属于同一 verification 目录".into());
    }
    let observed = common::read_bound(backend, descriptor, "暖来源短期键观察")?;
    validate_observation(&observed, receipt, before, facts, node)?;
    Ok(observed)
}

fn validate_observation(
    observed: &Value,
    receipt: &Value,
    before: &Value,
    facts: &Value,
    node: &Value,
) -> Result<()> {
    document::exact(
        observed,
        &[
            "format_version",
            "kind",
            "status",
            "started_at",
            "cleaned_at",
            "absent_at_ms",
            "preserved_authorization_keys",
            "expired_snapshot_keys",
            "expired_rate_limit_keys",
            "remote_writes",
            "source_generation",
            "before",
            "subjects_sha256",
            "sessions_sha256",
            "authorization_observation",
        ],
        "暖来源短期键自然过期证据",
    )?;
    let mut expected_binding = json!({});
    bind_observation(
        &mut expected_binding,
        &receipt["source_generation"],
        &receipt["before"],
        node,
    )?;
    if expected_binding
        .as_object()
        .is_none_or(|values| values.iter().any(|(key, value)| &observed[key] != value))
        || observed["format_version"] != 1
        || observed["kind"] != "restore-source-warm-redis-expiry"
        || observed["status"] != "observed_absent"
        || observed["remote_writes"] != 0
    {
        return Err("暖来源短期键证据没有绑定同一启动、完整像和会话".into());
    }
    let scope = document::text(
        &facts["lineage"]["verification"]["scope_id"],
        "暖来源 scope",
    )?;
    let namespace = format!("ryframe:{{{scope}}}:");
    let original = document::object(&before["redis"]["existing"], "暖来源 Redis 前像")?;
    check_authorization(observed, &namespace, original, &node["subjects"])?;
    let kinds = resources::warm_transient::expected(
        &namespace,
        &facts["lineage"]["tenants"],
        &node["subjects"],
    )?;
    let absent = observed["absent_at_ms"]
        .as_i64()
        .filter(|n| *n > 0)
        .ok_or("暖来源短期键缺失时间无效")?;
    let started = epoch_millis(&observed["started_at"])?;
    let finished = epoch_millis(&observed["cleaned_at"])?;
    if absent < started || absent > finished {
        return Err("Redis 短期键缺失时间超出本次实际观察窗口".into());
    }
    for (field, kind) in [
        ("expired_snapshot_keys", "hash"),
        ("expired_rate_limit_keys", "string"),
    ] {
        let rows = document::array(&observed[field], "暖来源已观察的自然过期键")?;
        check_expired(rows, kind, original, &kinds, started, absent)?;
    }
    Ok(())
}

fn check_authorization(
    observed: &Value,
    namespace: &str,
    original: &serde_json::Map<String, Value>,
    subjects: &Value,
) -> Result<()> {
    let authorization = cache::observation(
        &json!({"value":observed["authorization_observation"],"subjects":subjects}),
    )?;
    if authorization["namespace"] != namespace {
        return Err("暖来源授权缓存观察脱离原 namespace".into());
    }
    for row in document::array(&authorization["before"], "暖来源授权缓存读取")? {
        for field in ["tenant_epoch", "user_version"] {
            let key = document::text(&row[field]["key"], "暖来源原授权键")?;
            let value = document::text(&row[field]["value"], "暖来源原授权值")?;
            if original.get(key).is_none_or(|entry| {
                entry["kind"] != "string"
                    || entry["sha256"] != hex::encode(Sha256::digest(value.as_bytes()))
                    || !entry["expires_at_ms"].is_null()
            }) {
                return Err("暖来源原授权代次或用户版本在会话之后发生变化".into());
            }
        }
    }
    if observed["preserved_authorization_keys"]
        != original
            .keys()
            .filter(|name| {
                name.strip_prefix(namespace)
                    .is_some_and(|name| name.starts_with("authorization:"))
            })
            .count()
    {
        return Err("暖来源原授权键计数与完整前像不同".into());
    }
    Ok(())
}

fn check_expired(
    rows: &[Value],
    kind: &str,
    original: &serde_json::Map<String, Value>,
    plan: &std::collections::BTreeMap<String, (&'static str, i64)>,
    started: i64,
    absent: i64,
) -> Result<()> {
    let mut prior = None;
    for row in rows {
        document::exact(
            row,
            &["key", "observed_at_ms", "expires_at_ms"],
            "暖来源已观察过期键",
        )?;
        let key = document::text(&row["key"], "暖来源过期键")?;
        let (expected_kind, bounded_ttl) = plan.get(key).ok_or("暖来源过期键超出计划")?;
        let first = row["observed_at_ms"]
            .as_i64()
            .filter(|n| *n > 0)
            .ok_or("短期键首次观察时间无效")?;
        let deadline = row["expires_at_ms"].as_i64().ok_or("短期键绝对期限无效")?;
        if *expected_kind != kind
            || original.contains_key(key)
            || prior.is_some_and(|previous| previous >= key)
            || first < started
            || deadline <= first
            || deadline > absent
            || deadline > first.saturating_add(*bounded_ttl)
        {
            return Err("暖来源短期键原归属、类型、期限或自然过期证据不符".into());
        }
        prior = Some(key);
    }
    Ok(())
}

fn epoch_millis(value: &Value) -> Result<i64> {
    let (seconds, nanos) = rfc3339_order_key(document::text(value, "暖来源短期键观察时间")?)?;
    seconds
        .checked_mul(1_000)
        .and_then(|millis| millis.checked_add(i64::from(nanos / 1_000_000)))
        .ok_or_else(|| "暖来源短期键观察时间无法转换为 Unix 毫秒".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn warm_expiry_evidence_must_follow_the_observation_start() {
        let plan = BTreeMap::from([("example".to_owned(), ("hash", 300_000))]);
        let original = serde_json::Map::new();
        let mut rows = [json!({"key":"example","observed_at_ms":1_000,"expires_at_ms":2_000})];
        assert!(check_expired(&rows, "hash", &original, &plan, 999, 2_000).is_ok());
        assert!(check_expired(&rows, "hash", &original, &plan, 1_001, 2_000).is_err());
        rows[0]["expires_at_ms"] = json!(2_001);
        assert!(check_expired(&rows, "hash", &original, &plan, 999, 2_000).is_err());
        assert_eq!(
            epoch_millis(&json!("1970-01-01T08:00:01.123456+08:00")).unwrap(),
            1_123
        );
    }
}
