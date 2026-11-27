//! 仅对已登记来源代次观察原 Redis 内容与绝对到期时间。
use std::collections::BTreeMap;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::{Uuid, Variant};

use crate::{
    Result,
    recovery::clone::native::{
        digest,
        resources::common,
        target_verify::{
            redis::{self, Client},
            storage::{StorageInputs, redis_layout},
        },
    },
};

const GENERATION_ID: &str = "wkc61111h-current-20261127-r1";
const SOURCE_SHA256: &str = "4f084753d4c2b797d3aa73c967a3bab0d9051f8cc569536651e2eaed79166ff6";
const FAMILY_EVIDENCE_PREFIX: &str = ".observed-refresh-family-sha256:";
const MAX_KEYS: usize = 2_048;
const MAX_SCAN_PAGES: usize = 128;
const FAMILY_FIELDS: [&str; 9] = [
    "sid",
    "tenant_id",
    "user_id",
    "current_jti",
    "previous_jti",
    "rotated_at",
    "absolute_exp",
    "revoked",
    "last_attempt_id",
];

pub(in crate::recovery::source::evidence) fn applies(generation: &Value) -> bool {
    generation["format_version"].as_u64() == Some(2)
        && generation["id"] == GENERATION_ID
        && generation["source_registration"]["sha256"] == SOURCE_SHA256
}

pub(super) fn capture(inputs: &StorageInputs<'_>, redis: &Value) -> Result<Value> {
    redis_layout::require_windows(redis)?;
    let mut client = Client::authenticated(inputs, redis)?;
    if !redis::info_matches(redis, &client.transaction_command(&["INFO", "server"])?)? {
        return Err("来源 Redis 快照开始时的代次与登记不符".into());
    }
    let value = snapshot(inputs, &mut |parts| {
        #[cfg(windows)]
        if parts == ["EXEC"] {
            client.recheck_windows_peer()?;
        }
        client.transaction_command(parts)
    })?;
    if !redis::info_matches(redis, &client.transaction_command(&["INFO", "server"])?)? {
        return Err("来源 Redis 快照结束时的代次与登记不符".into());
    }
    #[cfg(windows)]
    client.recheck_windows_peer()?;
    Ok(value)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind<'a> {
    String,
    AuthorizationSnapshot,
    RefreshFamily(&'a str),
}

impl Kind<'_> {
    fn redis_type(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::AuthorizationSnapshot | Self::RefreshFamily(_) => "hash",
        }
    }
}

fn snapshot(
    inputs: &StorageInputs<'_>,
    command: &mut impl FnMut(&[&str]) -> Result<Value>,
) -> Result<Value> {
    let scope = inputs.target_scope()?;
    common::scope_id(scope)?;
    let namespace = format!("ryframe:{{{scope}}}:");
    let owner = format!("{namespace}.ryframe-owner");
    let sentinel = format!("ryframe:devex-fresh:{scope}:sentinel");
    let reset = &inputs.request["reset"];
    let expected_sentinel = reset["sentinel_value"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or("来源 Redis 快照缺少原哨兵值")?;
    if reset["sentinel_key"] != sentinel {
        return Err("来源 Redis 快照的哨兵键与原 scope 不符".into());
    }
    let mut image = read_markers(inputs, command)?;
    let keys = image["keys"].as_array().ok_or("来源 Redis 缺少键清单")?;
    if image["owner"] != format!("ryframe-owner:v1:{scope}:redis")
        || image["sentinel"] != expected_sentinel
        || !keys.iter().any(|key| key == owner.as_str())
    {
        return Err("来源 Redis 快照的归属标记与登记不符".into());
    }
    let members = keys
        .iter()
        .map(|key| {
            let key = key.as_str().ok_or("来源 Redis 键类型无效")?;
            let relative = key.strip_prefix(&namespace).ok_or("来源 Redis 键越界")?;
            let kind = key_kind(relative).ok_or("来源 Redis 快照含未登记的键类型")?;
            Ok((key, kind))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut watch = Vec::with_capacity(members.len() + 2);
    watch.push("WATCH");
    watch.extend(members.iter().map(|(key, _)| *key));
    watch.push(&sentinel);
    if command(&watch)? != "OK" || read_markers(inputs, command)? != image {
        return Err("来源 Redis 快照在建立监视时发生变化".into());
    }
    let now_ms = server_time_ms(&command(&["TIME"])?)?;
    read_expiry(command, &sentinel, Kind::String, now_ms)?;
    let mut existing = BTreeMap::new();
    for (key, kind) in members {
        let (name, entry) = read_entry(command, &namespace, key, kind, now_ms)?;
        if existing.insert(name, entry).is_some() {
            return Err("来源 Redis 快照的证据键重复".into());
        }
    }
    if read_markers(inputs, command)? != image {
        return Err("来源 Redis 快照在读取期间发生变化".into());
    }
    finish_watch(command)?;
    image["keys"] = json!(existing.keys().collect::<Vec<_>>());
    image["existing"] = json!(existing);
    Ok(image)
}

pub(super) fn read_markers(
    inputs: &StorageInputs<'_>,
    command: &mut impl FnMut(&[&str]) -> Result<Value>,
) -> Result<Value> {
    let mut pages = 0;
    let mut keys = 0;
    redis::marker_image(inputs, &mut |parts| {
        if parts.first() != Some(&"SCAN") {
            return command(parts);
        }
        pages += 1;
        if pages > MAX_SCAN_PAGES {
            return Err("来源 Redis 快照超过分页上限".into());
        }
        let response = command(parts)?;
        let page = response[1]
            .as_array()
            .ok_or("来源 Redis 快照的分页键清单无效")?;
        keys += page.len();
        if keys > MAX_KEYS {
            return Err("来源 Redis 快照超过键数量上限".into());
        }
        Ok(response)
    })
}

fn key_kind(relative: &str) -> Option<Kind<'_>> {
    if relative == ".ryframe-owner" {
        return Some(Kind::String);
    }
    if let Some(sid) = relative.strip_prefix("v0.5:refresh-family:") {
        let raw = sid.strip_prefix("s-")?;
        let uuid = Uuid::parse_str(raw).ok()?;
        return (uuid.get_version_num() == 4
            && uuid.get_variant() == Variant::RFC4122
            && uuid.hyphenated().to_string() == raw)
            .then_some(Kind::RefreshFamily(sid));
    }
    let authorization = relative.strip_prefix("authorization:{")?;
    let (tenant, entry) = authorization.split_once("}:")?;
    if !valid_tenant(tenant) {
        return None;
    }
    if entry == "epoch" {
        return Some(Kind::String);
    }
    let (user, kind) = entry.strip_prefix("user:")?.split_once(':')?;
    if decimal(user)? == 0 {
        return None;
    }
    match kind {
        "version" => Some(Kind::String),
        "snapshots:v4" => Some(Kind::AuthorizationSnapshot),
        _ => None,
    }
}

fn valid_tenant(id: &str) -> bool {
    let id = id.as_bytes();
    (2..=64).contains(&id.len())
        && id.first().is_some_and(u8::is_ascii_alphanumeric)
        && id.last().is_some_and(u8::is_ascii_alphanumeric)
        && id
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_'))
}

fn decimal(value: &str) -> Option<i64> {
    let number = value.parse::<i64>().ok()?;
    (number >= 0 && number.to_string() == value).then_some(number)
}

pub(super) fn server_time_ms(value: &Value) -> Result<i64> {
    let Some([seconds, micros]) = value.as_array().map(Vec::as_slice) else {
        return Err("来源 Redis 快照的服务器时钟响应无效".into());
    };
    let seconds = seconds
        .as_str()
        .and_then(decimal)
        .filter(|seconds| *seconds > 0)
        .ok_or("来源 Redis 快照的服务器秒时钟无效")?;
    let micros = micros
        .as_str()
        .and_then(decimal)
        .filter(|micros| *micros < 1_000_000)
        .ok_or("来源 Redis 快照的服务器微秒时钟无效")?;
    seconds
        .checked_mul(1_000)
        .and_then(|ms| ms.checked_add(micros / 1_000))
        .ok_or_else(|| "来源 Redis 快照的服务器时钟溢出".into())
}

fn read_expiry(
    command: &mut impl FnMut(&[&str]) -> Result<Value>,
    key: &str,
    kind: Kind<'_>,
    now_ms: i64,
) -> Result<Option<i64>> {
    if command(&["TYPE", key])? != kind.redis_type() {
        return Err("来源 Redis 快照的实际键类型与定义不符".into());
    }
    let expiry = command(&["PEXPIRETIME", key])?
        .as_i64()
        .ok_or("来源 Redis 快照的绝对到期响应不是整数")?;
    match kind {
        Kind::String if expiry == -1 => Ok(None),
        Kind::AuthorizationSnapshot | Kind::RefreshFamily(_) if expiry > now_ms => Ok(Some(expiry)),
        _ => Err("来源 Redis 快照的持久属性或绝对到期时间不符".into()),
    }
}

fn read_entry(
    command: &mut impl FnMut(&[&str]) -> Result<Value>,
    namespace: &str,
    key: &str,
    kind: Kind<'_>,
    now_ms: i64,
) -> Result<(String, Value)> {
    let expiry = read_expiry(command, key, kind, now_ms)?;
    let mut name = key.to_owned();
    let mut family = None;
    let sha256 = match kind {
        Kind::String => {
            let value = command(&["GET", key])?;
            let bytes = value.as_str().ok_or("来源 Redis 快照的字符串响应无效")?;
            hex::encode(Sha256::digest(bytes.as_bytes()))
        }
        Kind::AuthorizationSnapshot | Kind::RefreshFamily(_) => {
            let count = if kind == Kind::AuthorizationSnapshot {
                1
            } else {
                FAMILY_FIELDS.len()
            };
            if command(&["HLEN", key])?.as_u64() != Some(count as u64) {
                return Err("来源 Redis 快照的哈希字段数量与定义不符".into());
            }
            let value = command(&["HGETALL", key])?;
            let pairs = hash_pairs(&value, count)?;
            if let Kind::RefreshFamily(sid) = kind {
                let sid_sha256 = hex::encode(Sha256::digest(sid.as_bytes()));
                name = family_key(namespace, &sid_sha256);
                family = Some(family_observation(&pairs, sid, &sid_sha256, expiry)?);
            }
            digest::plan_hash(&json!(pairs.into_iter().collect::<Vec<_>>()))?
        }
    };
    let mut entry = json!({"kind":kind.redis_type(),"sha256":sha256,"expires_at_ms":expiry});
    if let Some(family) = family {
        entry["family"] = family;
    }
    Ok((name, entry))
}

pub(super) fn family_key(namespace: &str, sid_sha256: &str) -> String {
    format!("{namespace}{FAMILY_EVIDENCE_PREFIX}{sid_sha256}")
}

pub(super) fn observed_key(namespace: &str, key: &str) -> Result<String> {
    let relative = key.strip_prefix(namespace).ok_or("来源 Redis 键越界")?;
    if let Some(Kind::RefreshFamily(sid)) = key_kind(relative) {
        let digest = hex::encode(Sha256::digest(sid.as_bytes()));
        Ok(family_key(namespace, &digest))
    } else {
        Ok(key.to_owned())
    }
}

fn hash_pairs(value: &Value, count: usize) -> Result<BTreeMap<&str, &str>> {
    let fields = value
        .as_array()
        .filter(|fields| fields.len() == count * 2)
        .ok_or("来源 Redis 快照的哈希响应长度无效")?;
    let mut pairs = BTreeMap::new();
    for pair in fields.chunks_exact(2) {
        let field = pair[0]
            .as_str()
            .filter(|field| !field.is_empty())
            .ok_or("来源 Redis 快照的哈希字段无效")?;
        let value = pair[1]
            .as_str()
            .ok_or("来源 Redis 快照的哈希值不是字符串")?;
        if pairs.insert(field, value).is_some() {
            return Err("来源 Redis 快照的哈希字段重复".into());
        }
    }
    Ok(pairs)
}

fn family_observation(
    pairs: &BTreeMap<&str, &str>,
    sid: &str,
    sid_sha256: &str,
    expiry: Option<i64>,
) -> Result<Value> {
    if FAMILY_FIELDS.iter().any(|field| !pairs.contains_key(field))
        || pairs["sid"] != sid
        || !valid_tenant(pairs["tenant_id"])
    {
        return Err("来源 Redis 快照的刷新会话字段与定义不符".into());
    }
    let absolute_exp = decimal(pairs["absolute_exp"])
        .filter(|seconds| *seconds > 0)
        .ok_or("来源 Redis 快照的刷新会话到期时刻无效")?;
    if expiry.is_none() || absolute_exp.checked_mul(1_000) != expiry {
        return Err("来源 Redis 快照的刷新会话绝对到期绑定不符".into());
    }
    let user_id = decimal(pairs["user_id"])
        .filter(|id| *id > 0)
        .ok_or("来源 Redis 快照的刷新会话用户无效")?;
    let rotated_at = decimal(pairs["rotated_at"]).ok_or("来源 Redis 快照的刷新会话时刻无效")?;
    let revoked = match pairs["revoked"] {
        "0" => false,
        "1" => true,
        _ => return Err("来源 Redis 快照的刷新会话注销状态无效".into()),
    };
    Ok(json!({
        "tenant_id":pairs["tenant_id"],
        "user_id":user_id,
        "sid_sha256":sid_sha256,
        "rotated_at":rotated_at,
        "absolute_exp":absolute_exp,
        "revoked":revoked,
        "never_rotated":pairs["previous_jti"].is_empty() && pairs["last_attempt_id"].is_empty(),
    }))
}

fn finish_watch(command: &mut impl FnMut(&[&str]) -> Result<Value>) -> Result<()> {
    // 单纯 PING 使 EXEC 给出 WATCH 结果，连接不提交任何键写操作。
    if command(&["MULTI"])? != "OK"
        || command(&["PING"])? != "QUEUED"
        || command(&["EXEC"])? != json!(["PONG"])
    {
        return Err("来源 Redis 快照的只读 WATCH 未确认".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const OWNER: &str = "ryframe:{safe-scope}:.ryframe-owner";
    const OWNER_VALUE: &str = "ryframe-owner:v1:safe-scope:redis";
    const SENTINEL: &str = "ryframe:devex-fresh:safe-scope:sentinel";
    const VERSION: &str = "ryframe:{safe-scope}:authorization:{test-team}:user:35:version";
    const SNAPSHOT: &str = "ryframe:{safe-scope}:authorization:{test-team}:user:35:snapshots:v4";
    const SID: &str = "s-66d87deb-c2c0-4664-b7d7-be4b259dd746";
    const FAMILY: &str =
        "ryframe:{safe-scope}:v0.5:refresh-family:s-66d87deb-c2c0-4664-b7d7-be4b259dd746";
    const SID_SHA256: &str = "8d86323830ef7a5aaa16605d18a71106b8fb2cab08d7d56c2d6e905d9a084941";
    const EXPIRES_MS: i64 = 1_700_000_600_000;

    #[test]
    fn warm_redis_requires_exact_v2_generation_and_source() {
        let mut generation = json!({
            "format_version": 2, "id": GENERATION_ID,
            "source_registration": {"sha256": SOURCE_SHA256}
        });
        assert!(applies(&generation));
        generation["format_version"] = json!(1);
        assert!(!applies(&generation));
        generation["format_version"] = json!(2);
        generation["id"] = json!("other");
        assert!(!applies(&generation));
        generation["id"] = json!(GENERATION_ID);
        generation["source_registration"]["sha256"] = json!(SOURCE_SHA256.to_uppercase());
        assert!(!applies(&generation));
    }

    #[test]
    fn warm_redis_matches_only_frozen_product_key_grammar() {
        let prefix = "ryframe:{safe-scope}:";
        assert_eq!(
            key_kind(OWNER.strip_prefix(prefix).unwrap()),
            Some(Kind::String)
        );
        assert_eq!(
            key_kind(VERSION.strip_prefix(prefix).unwrap()),
            Some(Kind::String)
        );
        assert_eq!(
            key_kind(SNAPSHOT.strip_prefix(prefix).unwrap()),
            Some(Kind::AuthorizationSnapshot)
        );
        assert_eq!(
            key_kind(&format!("v0.5:refresh-family:{SID}")),
            Some(Kind::RefreshFamily(SID))
        );
        for key in [
            "authorization:{-invalid}:epoch",
            "authorization:{test-team}:user:035:version",
            "authorization:{test-team}:user:35:snapshots:v3",
            "v0.5:refresh-family:s-66d87deb-c2c0-1664-b7d7-be4b259dd746",
            "v0.5:refresh-family:s-66D87DEB-c2c0-4664-b7d7-be4b259dd746",
            "v0.5:refresh-family-index:tenant:test-team",
        ] {
            assert!(key_kind(key).is_none(), "{key}");
        }
    }

    #[test]
    fn warm_redis_hash_rejects_duplicate_fields_and_checks_family_absolute_deadline() {
        let first = json!(["second", "value-b", "first", "value-a"]);
        let reordered = json!(["first", "value-a", "second", "value-b"]);
        assert_eq!(
            hash_pairs(&first, 2).unwrap(),
            hash_pairs(&reordered, 2).unwrap()
        );
        assert!(hash_pairs(&json!(["duplicate", "a", "duplicate", "b"]), 2).is_err());
        assert!(hash_pairs(&json!(["first", 7]), 1).is_err());
        let value = fixture_family();
        let mut family = hash_pairs(&value, FAMILY_FIELDS.len()).unwrap();
        assert_eq!(
            family_observation(&family, SID, SID_SHA256, Some(EXPIRES_MS)).unwrap()["never_rotated"]
                .as_bool(),
            Some(true)
        );
        assert!(family_observation(&family, SID, SID_SHA256, Some(EXPIRES_MS + 1)).is_err());
        assert!(family_observation(&family, "other", SID_SHA256, Some(EXPIRES_MS)).is_err());
        for field in ["previous_jti", "last_attempt_id"] {
            family.insert(field, "non-empty");
            assert_eq!(
                family_observation(&family, SID, SID_SHA256, Some(EXPIRES_MS)).unwrap()
                    ["never_rotated"]
                    .as_bool(),
                Some(false)
            );
            family.insert(field, "");
        }
    }

    #[test]
    fn warm_redis_uses_read_only_watch_and_rejects_its_abort_or_inventory_drift() {
        let result = fixture_capture(json!(["PONG"]), false).unwrap();
        let evidence = format!("ryframe:{{safe-scope}}:{FAMILY_EVIDENCE_PREFIX}{SID_SHA256}");
        assert_eq!(
            result["keys"],
            json!([evidence.as_str(), OWNER, SNAPSHOT, VERSION])
        );
        assert_eq!(result["owner"], OWNER_VALUE);
        assert_eq!(result["sentinel"], "fixture-sentinel");
        let entries = &result["existing"];
        assert_eq!(entries[VERSION]["kind"], "string");
        assert_eq!(entries[VERSION]["expires_at_ms"], Value::Null);
        assert_eq!(entries[SNAPSHOT]["kind"], "hash");
        assert_eq!(entries[SNAPSHOT]["expires_at_ms"], 1_700_000_005_123_i64);
        assert_eq!(
            entries[SNAPSHOT]["sha256"],
            digest::plan_hash(&json!([["3:4", "private-test-payload"]])).unwrap()
        );
        assert_eq!(
            entries[evidence.as_str()]["family"],
            json!({
                "tenant_id":"test-team","user_id":35,"sid_sha256":SID_SHA256,
                "rotated_at":1_700_000_001_i64,"absolute_exp":1_700_000_600_i64,
                "revoked":true,"never_rotated":true
            })
        );
        assert_eq!(entries[evidence.as_str()]["expires_at_ms"], EXPIRES_MS);
        assert_eq!(
            result["keys"],
            json!(entries.as_object().unwrap().keys().collect::<Vec<_>>())
        );
        let serialized = result.to_string();
        for raw in [FAMILY, SID, "private-test-payload", "private-current-jti"] {
            assert!(!serialized.contains(raw));
        }
        assert!(fixture_capture(Value::Null, false).is_err());
        assert!(fixture_capture(json!(["PONG"]), true).is_err());
    }

    fn fixture_capture(exec: Value, drift: bool) -> Result<Value> {
        let request = json!({
            "target": {"scope_id":"safe-scope"},
            "reset": {"sentinel_key":SENTINEL, "sentinel_value":"fixture-sentinel"}
        });
        let inputs = StorageInputs {
            backend: Path::new("."),
            request: &request,
            environment: &BTreeMap::new(),
            observation_dir: Path::new("."),
        };
        let mut scans = 0;
        let mut transaction = 0;
        snapshot(&inputs, &mut |parts| {
            let response = match parts {
                [
                    "SCAN",
                    "0",
                    "MATCH",
                    "ryframe:{safe-scope}:*",
                    "COUNT",
                    "1000",
                ] => {
                    scans += 1;
                    if drift && scans == 3 {
                        return Ok(json!(["0", [OWNER, VERSION, FAMILY]]));
                    }
                    json!(["0", [VERSION, OWNER, FAMILY, SNAPSHOT]])
                }
                ["GET", OWNER] => json!(OWNER_VALUE),
                ["GET", SENTINEL] => json!("fixture-sentinel"),
                ["GET", VERSION] => json!("4"),
                ["WATCH", OWNER, SNAPSHOT, VERSION, FAMILY, SENTINEL] => {
                    assert_eq!(scans, 1);
                    json!("OK")
                }
                ["TIME"] => json!(["1700000000", "8835"]),
                ["TYPE", OWNER | SENTINEL | VERSION] => json!("string"),
                ["TYPE", SNAPSHOT | FAMILY] => json!("hash"),
                ["PEXPIRETIME", OWNER | SENTINEL | VERSION] => json!(-1),
                ["PEXPIRETIME", SNAPSHOT] => json!(1_700_000_005_123_i64),
                ["PEXPIRETIME", FAMILY] => json!(EXPIRES_MS),
                ["HLEN", SNAPSHOT] => json!(1),
                ["HLEN", FAMILY] => json!(FAMILY_FIELDS.len()),
                ["HGETALL", SNAPSHOT] => json!(["3:4", "private-test-payload"]),
                ["HGETALL", FAMILY] => fixture_family(),
                ["MULTI"] => {
                    assert_eq!(scans, 3);
                    transaction = 1;
                    json!("OK")
                }
                ["PING"] if transaction == 1 => {
                    transaction = 2;
                    json!("QUEUED")
                }
                ["EXEC"] if transaction == 2 => exec.clone(),
                _ => panic!("unexpected mock command {parts:?}"),
            };
            Ok(response)
        })
    }

    fn fixture_family() -> Value {
        json!([
            "tenant_id",
            "test-team",
            "sid",
            SID,
            "user_id",
            "35",
            "rotated_at",
            "1700000001",
            "absolute_exp",
            "1700000600",
            "revoked",
            "1",
            "current_jti",
            "private-current-jti",
            "previous_jti",
            "",
            "last_attempt_id",
            ""
        ])
    }
}
