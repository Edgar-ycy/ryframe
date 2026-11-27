//! 校验暖机只留下本次生产者的已撤销刷新会话，保留原 Redis 前后像。
use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    Result,
    recovery::clone::native::{json as document, resources::common},
    utc_timestamp::rfc3339_order_key,
};

const OWNER_KEY: &str = ".ryframe-owner";
const FAMILY_PREFIX: &str = ".observed-refresh-family-sha256:";
const SESSION_COUNT: usize = 11;
const LEASE_SECONDS: i64 = 604_800;

type Actor<'a> = (&'a str, i64);

struct Image<'a> {
    namespace: String,
    owner: &'a str,
    sentinel: &'a str,
    entries: BTreeMap<&'a str, &'a Value>,
}

struct Family<'a> {
    actor: Actor<'a>,
    sid_digest: &'a str,
    rotated_at: i64,
    absolute_exp: i64,
    revoked: bool,
    never_rotated: bool,
}

pub(super) fn compare(
    before: &Value,
    after: &Value,
    session_subjects: &Value,
    started_at: &str,
    finished_at: &str,
) -> Result<Value> {
    let before = Image::read(before)?;
    let after = Image::read(after)?;
    if before.owner != after.owner || before.sentinel != after.sentinel {
        return Err("暖机期间 Redis 归属标记或哨兵变化".into());
    }
    let mut subjects = producer_subjects(session_subjects)?;
    let (started, finished) = (
        rfc3339_order_key(started_at).map_err(|_| "暖机生产者开始时间无效")?,
        rfc3339_order_key(finished_at).map_err(|_| "暖机生产者结束时间无效")?,
    );
    if started.0 <= 0
        || started > finished
        || started.1 >= 1_000_000_000
        || finished.1 >= 1_000_000_000
    {
        return Err("暖机生产者真实运行时间范围无效".into());
    }
    for (&key, &original) in &before.entries {
        let family = entry(key, original, &before.namespace)?;
        if after.entries.get(key).copied() != Some(original) {
            return Err("暖机期间已有 Redis 条目缺失或内容、类型、期限发生变化".into());
        }
        if family.is_some_and(|family| subjects.contains_key(family.sid_digest)) {
            return Err("暖机生产者会话已出现在 Redis 前像，禁止重复接续".into());
        }
    }
    let mut retained = Vec::with_capacity(SESSION_COUNT);
    for (&key, &value) in &after.entries {
        if before.entries.contains_key(key) {
            continue;
        }
        let family = entry(key, value, &after.namespace)?
            .ok_or("暖机新增 Redis 条目不属于已登记刷新会话")?;
        // 认证端记录 Unix 整秒；同一秒内的亚秒部分由生产者时间界限核定。
        if subjects.remove(family.sid_digest) != Some(family.actor)
            || !(started.0..=finished.0).contains(&family.rotated_at)
            || family.absolute_exp.checked_sub(family.rotated_at) != Some(LEASE_SECONDS)
            || !family.revoked
            || !family.never_rotated
        {
            return Err("暖机新增会话的主体、创建时间、原租期或撤销状态不符".into());
        }
        retained.push(json!({
            "key":key,"sha256":value["sha256"],"expires_at_ms":value["expires_at_ms"],
            "tenant_id":family.actor.0,"user_id":family.actor.1.to_string(),
            "sid_sha256":family.sid_digest,"rotated_at":family.rotated_at,"absolute_exp":family.absolute_exp,
            "revoked":family.revoked,"never_rotated":family.never_rotated,
        }));
    }
    if !subjects.is_empty() || retained.len() != SESSION_COUNT {
        return Err("暖机 Redis 后像缺少完整的生产者撤销会话集合".into());
    }
    Ok(json!({"preserved_keys":before.entries.len(),"retained_revoked_sessions":retained}))
}

impl<'a> Image<'a> {
    fn read(value: &'a Value) -> Result<Self> {
        document::exact(
            value,
            &["keys", "owner", "sentinel", "existing"],
            "暖机 Redis 完整像",
        )?;
        let owner = document::text(&value["owner"], "暖机 Redis 归属标记")?;
        let scope = owner
            .strip_prefix("ryframe-owner:v1:")
            .and_then(|value| value.strip_suffix(":redis"))
            .ok_or("暖机 Redis 归属标记格式无效")?;
        common::scope_id(scope)?;
        let namespace = format!("ryframe:{{{scope}}}:");
        let sentinel = document::text(&value["sentinel"], "暖机 Redis 哨兵")?;
        if sentinel.is_empty() {
            return Err("暖机 Redis 哨兵不能为空".into());
        }
        let keys = document::array(&value["keys"], "暖机 Redis 键清单")?;
        let entries = document::object(&value["existing"], "暖机 Redis 条目")?
            .iter()
            .map(|(key, entry)| (key.as_str(), entry))
            .collect::<BTreeMap<_, _>>();
        if keys.len() != entries.len()
            || keys
                .iter()
                .zip(entries.keys())
                .any(|(listed, key)| listed.as_str() != Some(*key))
        {
            return Err("暖机 Redis 键清单必须精确、唯一且按键名排序".into());
        }
        let owner_key = format!("{namespace}{OWNER_KEY}");
        let owner_entry = entries
            .get(owner_key.as_str())
            .ok_or("暖机 Redis 完整像缺少归属键")?;
        if owner_entry["sha256"] != hex::encode(Sha256::digest(owner.as_bytes())) {
            return Err("暖机 Redis 归属键内容与归属标记不同".into());
        }
        Ok(Self {
            namespace,
            owner,
            sentinel,
            entries,
        })
    }
}

fn producer_subjects(value: &Value) -> Result<BTreeMap<&str, Actor<'_>>> {
    let records = document::array(value, "暖机生产者会话")?;
    if records.len() != SESSION_COUNT {
        return Err("暖机生产者会话数量不符".into());
    }
    let mut actors = BTreeSet::new();
    let mut subjects = BTreeMap::new();
    for record in records {
        document::exact(
            record,
            &["tenant_id", "user_id", "sid_sha256"],
            "暖机生产者会话主体",
        )?;
        let tenant = tenant(&record["tenant_id"])?;
        let user = record["user_id"]
            .as_str()
            .and_then(positive_decimal)
            .ok_or("暖机生产者用户 ID 不是规范十进制字符串")?;
        let actor = (tenant, user);
        if !actors.insert(actor)
            || subjects
                .insert(hash(&record["sid_sha256"])?, actor)
                .is_some()
        {
            return Err("暖机生产者主体或会话摘要重复".into());
        }
    }
    Ok(subjects)
}

fn entry<'a>(key: &str, value: &'a Value, namespace: &str) -> Result<Option<Family<'a>>> {
    let relative = key
        .strip_prefix(namespace)
        .ok_or("暖机 Redis 条目超出已登记命名空间")?;
    if let Some(sid) = relative.strip_prefix(FAMILY_PREFIX) {
        document::exact(
            value,
            &["kind", "sha256", "expires_at_ms", "family"],
            "暖机 Redis 刷新会话条目",
        )?;
        hash(&value["sha256"])?;
        let family = family(&value["family"])?;
        let expected = family
            .absolute_exp
            .checked_mul(1_000)
            .ok_or("暖机 Redis 刷新会话到期时间溢出")?;
        if value["kind"] != "hash"
            || value["expires_at_ms"].as_i64() != Some(expected)
            || sid != family.sid_digest
        {
            return Err("暖机 Redis 刷新会话的类型、到期或摘要键绑定无效".into());
        }
        return Ok(Some(family));
    }

    document::exact(
        value,
        &["kind", "sha256", "expires_at_ms"],
        "暖机 Redis 原条目",
    )?;
    hash(&value["sha256"])?;
    let kind = ordinary_kind(relative).ok_or("暖机 Redis 完整像含未登记的键")?;
    let expiry = &value["expires_at_ms"];
    if value["kind"] != kind
        || match kind {
            "string" => !expiry.is_null(),
            _ => !expiry.as_i64().is_some_and(|ms| ms > 0),
        }
    {
        return Err("暖机 Redis 原条目类型或到期时间无效".into());
    }
    Ok(None)
}

fn ordinary_kind(relative: &str) -> Option<&'static str> {
    if relative == OWNER_KEY {
        return Some("string");
    }
    let (tenant, entry) = relative.strip_prefix("authorization:{")?.split_once("}:")?;
    if !valid_tenant(tenant) {
        return None;
    }
    if entry == "epoch" {
        return Some("string");
    }
    let (user, kind) = entry.strip_prefix("user:")?.split_once(':')?;
    positive_decimal(user)?;
    match kind {
        "version" => Some("string"),
        "snapshots:v4" => Some("hash"),
        _ => None,
    }
}

fn family(value: &Value) -> Result<Family<'_>> {
    document::exact(
        value,
        &[
            "tenant_id",
            "user_id",
            "sid_sha256",
            "rotated_at",
            "absolute_exp",
            "revoked",
            "never_rotated",
        ],
        "暖机刷新会话投影",
    )?;
    let user = value["user_id"]
        .as_i64()
        .filter(|id| *id > 0)
        .ok_or("暖机刷新会话用户 ID 无效")?;
    let rotated_at = value["rotated_at"]
        .as_i64()
        .filter(|seconds| *seconds >= 0)
        .ok_or("暖机刷新会话创建时间无效")?;
    let absolute_exp = value["absolute_exp"]
        .as_i64()
        .filter(|seconds| *seconds > rotated_at)
        .ok_or("暖机刷新会话绝对到期时间无效")?;
    Ok(Family {
        actor: (tenant(&value["tenant_id"])?, user),
        sid_digest: hash(&value["sid_sha256"])?,
        rotated_at,
        absolute_exp,
        revoked: document::boolean(&value["revoked"], "暖机刷新会话撤销状态")?,
        never_rotated: document::boolean(&value["never_rotated"], "暖机刷新会话轮转状态")?,
    })
}

fn hash(value: &Value) -> Result<&str> {
    value
        .as_str()
        .filter(|text| {
            text.len() == 64
                && text
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or_else(|| "暖机 Redis SHA-256 摘要无效".into())
}

fn tenant(value: &Value) -> Result<&str> {
    value
        .as_str()
        .filter(|text| valid_tenant(text))
        .ok_or_else(|| "暖机刷新会话租户无效".into())
}

fn valid_tenant(tenant: &str) -> bool {
    let bytes = tenant.as_bytes();
    (2..=64).contains(&bytes.len())
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn positive_decimal(text: &str) -> Option<i64> {
    let value = text.parse::<i64>().ok()?;
    (value > 0 && value.to_string() == text).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NS: &str = "ryframe:{test-scope}:";
    const START: &str = "1970-01-01T00:16:40.500Z";
    const FINISH: &str = "1970-01-01T00:16:42.125Z";
    const FIRST: i64 = 1_000;

    fn key(relative: &str) -> String {
        format!("{NS}{relative}")
    }

    fn family_key(digest: &str) -> String {
        key(&format!("{FAMILY_PREFIX}{digest}"))
    }

    fn refresh(tenant: &str, user: i64, sid: &str, created: i64) -> Value {
        let expires = created + LEASE_SECONDS;
        json!({"kind":"hash","sha256":"a".repeat(64),"expires_at_ms":expires * 1_000,
            "family":{"tenant_id":tenant,"user_id":user,"sid_sha256":sid,"rotated_at":created,
                "absolute_exp":expires,"revoked":true,"never_rotated":true}})
    }

    fn list_keys(image: &mut Value) {
        let mut keys = image["existing"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        image["keys"] = json!(keys);
    }

    fn fixture() -> (Value, Value, Value) {
        let owner = "ryframe-owner:v1:test-scope:redis";
        let mut before =
            json!({"keys":[],"owner":owner,"sentinel":"original-sentinel","existing":{}});
        before["existing"][key(OWNER_KEY)] = json!({"kind":"string","expires_at_ms":null,
            "sha256":hex::encode(Sha256::digest(owner.as_bytes()))});
        before["existing"][key("authorization:{tenant-01}:epoch")] =
            json!({"kind":"string","expires_at_ms":null,"sha256":"b".repeat(64)});
        before["existing"][key("authorization:{tenant-01}:user:1:snapshots:v4")] =
            json!({"kind":"hash","expires_at_ms":9_999_999,"sha256":"c".repeat(64)});
        before["existing"][family_key(&"d".repeat(64))] =
            refresh("old-tenant", 91, &"d".repeat(64), 940);
        list_keys(&mut before);
        let mut after = before.clone();
        let mut subjects = Vec::new();
        for user in (1..=SESSION_COUNT as i64).rev() {
            let tenant = format!("tenant-{user:02}");
            let sid = format!("{user:064x}");
            after["existing"][family_key(&sid)] = refresh(&tenant, user, &sid, FIRST + user % 3);
            subjects.push(json!({"tenant_id":tenant,"user_id":user.to_string(),"sid_sha256":sid}));
        }
        list_keys(&mut after);
        (before, after, json!(subjects))
    }

    #[test]
    fn warm_effects_preserve_old_keys_and_bind_exact_eleven_terminal_sessions() {
        let (before, after, subjects) = fixture();
        let result = compare(&before, &after, &subjects, START, FINISH).unwrap();
        assert_eq!(result["preserved_keys"], 4);
        let retained = result["retained_revoked_sessions"].as_array().unwrap();
        assert_eq!(retained.len(), SESSION_COUNT);
        assert_eq!(retained[0]["user_id"], "1");
        assert_eq!(retained[0]["sid_sha256"], format!("{:064x}", 1));
        assert_eq!(
            retained[0]["expires_at_ms"],
            (FIRST + 1 + LEASE_SECONDS) * 1_000
        );
        assert_eq!(retained[10]["user_id"], "11");
        assert!(compare(&before, &after, &subjects, FINISH, START).is_err());
    }

    #[test]
    fn warm_effects_reject_legacy_or_drifting_originals_and_unknown_keys() {
        let (before, after, subjects) = fixture();
        let original = key("authorization:{tenant-01}:user:1:snapshots:v4");
        let old_family = family_key(&"d".repeat(64));
        for (field, value) in [
            ("expires_at_ms", json!(9_999_998)),
            ("sha256", json!("f".repeat(64))),
            ("kind", json!("string")),
        ] {
            let mut drift = after.clone();
            drift["existing"][&original][field] = value;
            assert!(compare(&before, &drift, &subjects, START, FINISH).is_err());
        }
        let mut drift = after.clone();
        drift["existing"][&old_family]["family"]["tenant_id"] = json!("foreign-tenant");
        assert!(compare(&before, &drift, &subjects, START, FINISH).is_err());
        let mut missing = after.clone();
        missing["existing"]
            .as_object_mut()
            .unwrap()
            .remove(&original);
        list_keys(&mut missing);
        assert!(compare(&before, &missing, &subjects, START, FINISH).is_err());
        let mut unsorted = after.clone();
        unsorted["keys"].as_array_mut().unwrap().swap(0, 1);
        assert!(compare(&before, &unsorted, &subjects, START, FINISH).is_err());
        let mut unknown = after.clone();
        unknown["existing"][key("v0.5:refresh-family-index:tenant:tenant-01")] = json!({});
        list_keys(&mut unknown);
        assert!(compare(&before, &unknown, &subjects, START, FINISH).is_err());
        let mut legacy = before.clone();
        legacy.as_object_mut().unwrap().remove("existing");
        assert!(compare(&legacy, &after, &subjects, START, FINISH).is_err());
        let mut marker = after;
        marker["sentinel"] = json!("replacement");
        assert!(compare(&before, &marker, &subjects, START, FINISH).is_err());
        marker["sentinel"] = json!("");
        assert!(Image::read(&marker).is_err());
    }

    #[test]
    fn warm_effects_reject_out_of_window_lease_drift_or_reused_producer_sessions() {
        let (before, after, subjects) = fixture();
        let first = family_key(&format!("{:064x}", 1));
        for (field, replacement) in [
            ("tenant_id", json!("wrong-actor")),
            ("user_id", json!(2)),
            ("revoked", json!(false)),
            ("never_rotated", json!(false)),
            ("sid_sha256", json!("0".repeat(63))),
        ] {
            let mut drift = after.clone();
            drift["existing"][&first]["family"][field] = replacement;
            assert!(compare(&before, &drift, &subjects, START, FINISH).is_err());
        }
        let sid = format!("{:064x}", 1);
        let mut drift = after.clone();
        drift["existing"][&first] = refresh("tenant-01", 1, &sid, FIRST - 1);
        assert!(compare(&before, &drift, &subjects, START, FINISH).is_err());
        drift["existing"][&first] = refresh("tenant-01", 1, &sid, FIRST + 3);
        assert!(compare(&before, &drift, &subjects, START, FINISH).is_err());
        drift["existing"][&first] = after["existing"][&first].clone();
        let stretched = FIRST + 2 + LEASE_SECONDS;
        drift["existing"][&first]["family"]["absolute_exp"] = json!(stretched);
        drift["existing"][&first]["expires_at_ms"] = json!(stretched * 1_000);
        assert!(compare(&before, &drift, &subjects, START, FINISH).is_err());
        let mut reused = before.clone();
        reused["existing"][&first] = after["existing"][&first].clone();
        list_keys(&mut reused);
        assert!(compare(&reused, &after, &subjects, START, FINISH).is_err());
        let mut changed = subjects.clone();
        changed[0]["sid_sha256"] = subjects[1]["sid_sha256"].clone();
        assert!(compare(&before, &after, &changed, START, FINISH).is_err());
        changed = subjects.clone();
        changed[0]["tenant_id"] = subjects[1]["tenant_id"].clone();
        changed[0]["user_id"] = subjects[1]["user_id"].clone();
        assert!(compare(&before, &after, &changed, START, FINISH).is_err());
        changed = subjects;
        changed[0]["user_id"] = json!("011");
        assert!(compare(&before, &after, &changed, START, FINISH).is_err());
    }
}
