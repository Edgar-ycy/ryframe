//! 只读观察已登记身份的单个 Redis 会话，不保存会话标识或认证材料。

use ryframe_config::ResourceScopeId;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::net::IpAddr;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::{Uuid, Variant};

use crate::{
    Result,
    recovery::clone::native::target_verify::storage::{
        StorageInputs, StorageObserver, SystemObserver,
    },
};

pub(super) struct Subject<'a> {
    pub(super) scope: &'a str,
    pub(super) tenant: &'a str,
    pub(super) user_id: i64,
    pub(super) username: &'a str,
    pub(super) client_address: &'a str,
    pub(super) login_epoch: i64,
    pub(super) sid_sha256: &'a str,
}

pub(super) struct Readback {
    pub(super) sid: String,
    pub(super) proof: Value,
    pub(super) active: bool,
    pub(super) cleared: bool,
}

pub(super) fn observe(
    context: &StorageInputs<'_>,
    redis: &Value,
    subject: &Subject<'_>,
    sid: Option<&str>,
) -> Result<Readback> {
    if context
        .request
        .pointer("/target/scope_id")
        .and_then(Value::as_str)
        != Some(subject.scope)
    {
        return Err("会话核对不属于当前隔离目标".into());
    }
    let keys = Keys::new(subject)?;
    if let Some(sid) = sid {
        require_sid(subject, sid)?;
    }
    SystemObserver.redis_window(context, redis, |read| {
        read_window(read, &keys, subject, sid)
    })
}

struct Keys {
    namespace: String,
    indexes: [String; 4],
}

impl Keys {
    fn new(subject: &Subject<'_>) -> Result<Self> {
        let scope = ResourceScopeId::parse(subject.scope)?;
        let tenant = subject.tenant;
        let user = subject.user_id;
        if !(2..=64).contains(&tenant.len())
            || !tenant
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !tenant
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !tenant
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || user <= 0
            || subject.username.is_empty()
            || subject.client_address.parse::<IpAddr>().is_err()
            || subject.login_epoch <= 0
            || subject.sid_sha256.len() != 64
            || !subject
                .sid_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("会话核对缺少明确的原身份或会话摘要".into());
        }
        let namespace = scope.redis_namespace();
        let indexes = [
            format!("{namespace}v0.5:refresh-family-index:tenant:{tenant}"),
            format!("{namespace}v0.5:refresh-family-index:tenant-user:{tenant}:{user}"),
            format!("{namespace}online-user:index:tenant:{tenant}"),
            format!("{namespace}online-user:index:user:{tenant}:{user}"),
        ];
        Ok(Self { namespace, indexes })
    }
}

type ReadCommand<'a> = dyn FnMut(&[&str]) -> Result<Value> + 'a;

#[derive(PartialEq)]
struct Snapshot {
    sid: String,
    family: Option<Family>,
    metadata: Option<OnlineMetadata>,
    indexes: [bool; 4],
}

#[derive(PartialEq)]
struct Family {
    absolute_exp: i64,
    rotated_at: i64,
    revoked: bool,
    current_jti_sha256: String,
}

#[derive(PartialEq)]
struct OnlineMetadata {
    absolute_exp: i64,
    expires: i64,
    last_access: i64,
    sha256: String,
}

fn read_window(
    read: &mut ReadCommand<'_>,
    keys: &Keys,
    subject: &Subject<'_>,
    sid: Option<&str>,
) -> Result<Readback> {
    let before = redis_time(read)?;
    let first = snapshot(read, keys, subject, sid)?;
    let second = snapshot(read, keys, subject, Some(&first.sid))?;
    let after = redis_time(read)?;
    if first != second || before > after || subject.login_epoch > before.0 {
        return Err("会话在同一 Redis 核对窗口内变化".into());
    }
    report(second, subject, after.0, sid.is_none())
}

fn snapshot(
    read: &mut ReadCommand<'_>,
    keys: &Keys,
    subject: &Subject<'_>,
    sid: Option<&str>,
) -> Result<Snapshot> {
    let refresh_user = user_member(read, &keys.indexes[1])?;
    let online_user = user_member(read, &keys.indexes[3])?;
    let indexed = indexed_sid(refresh_user.as_deref(), online_user.as_deref())?;
    let sid = sid.or(indexed).ok_or("原身份没有已登记的 Redis 会话")?;
    if indexed.is_some_and(|member| member != sid) {
        return Err("用户索引包含其他会话，保留现场".into());
    }
    require_sid(subject, sid)?;

    let family_key = format!("{}v0.5:refresh-family:{sid}", keys.namespace);
    let online_key = format!(
        "{}online-user:session:{}:{sid}",
        keys.namespace, subject.tenant
    );
    let family = read_family(read, &family_key, subject, sid)?;
    let metadata = read_metadata(read, &online_key, subject, sid)?;
    if let (Some(family), Some(metadata)) = (&family, &metadata)
        && family.absolute_exp != metadata.absolute_exp
    {
        return Err("会话族与在线元数据的过期代次不同".into());
    }
    let mut indexes = [false; 4];
    for (membership, key) in indexes.iter_mut().zip(&keys.indexes) {
        let present = key_type(read, key, "set")?;
        *membership = match redis_integer(&read(&["SISMEMBER", key, sid])?)? {
            0 => false,
            1 if present => true,
            _ => return Err("Redis 会话索引返回了未知成员状态".into()),
        };
    }
    if indexes[1] != refresh_user.is_some() || indexes[3] != online_user.is_some() {
        return Err("用户索引在会话核对期间变化".into());
    }
    Ok(Snapshot {
        sid: sid.to_owned(),
        family,
        metadata,
        indexes,
    })
}

fn indexed_sid<'a>(refresh: Option<&'a str>, online: Option<&'a str>) -> Result<Option<&'a str>> {
    if let (Some(refresh), Some(online)) = (refresh, online)
        && refresh != online
    {
        return Err("用户索引存在多个会话，保留现场".into());
    }
    Ok(refresh.or(online))
}

fn user_member(read: &mut ReadCommand<'_>, key: &str) -> Result<Option<String>> {
    let present = key_type(read, key, "set")?;
    match redis_integer(&read(&["SCARD", key])?)? {
        0 if !present => Ok(None),
        1 if present => {
            let members = read(&["SMEMBERS", key])?;
            let Some([Value::String(member)]) = members.as_array().map(Vec::as_slice) else {
                return Err("Redis 用户索引成员无法与基数绑定".into());
            };
            Ok(Some(member.clone()))
        }
        _ => Err("Redis 用户索引出现多个成员或结构变化，保留现场".into()),
    }
}

fn read_family(
    read: &mut ReadCommand<'_>,
    key: &str,
    subject: &Subject<'_>,
    sid: &str,
) -> Result<Option<Family>> {
    let present = key_type(read, key, "hash")?;
    let fields = read(&[
        "HMGET",
        key,
        "sid",
        "tenant_id",
        "user_id",
        "current_jti",
        "rotated_at",
        "absolute_exp",
        "revoked",
    ])?;
    let expires = redis_integer(&read(&["EXPIRETIME", key])?)?;
    if !present
        && expires == -2
        && fields
            .as_array()
            .is_some_and(|fields| fields.len() == 7 && fields.iter().all(Value::is_null))
    {
        return Ok(None);
    }
    let Some(
        [
            Value::String(actual),
            Value::String(tenant),
            user,
            Value::String(jti),
            rotated,
            absolute,
            Value::String(revoked),
        ],
    ) = fields.as_array().map(Vec::as_slice)
    else {
        return Err("Redis 会话族字段缺失或结构异常".into());
    };
    let user = redis_integer(user)?;
    let rotated_at = redis_integer(rotated)?;
    let absolute_exp = redis_integer(absolute)?;
    let revoked = match revoked.as_str() {
        "0" => false,
        "1" => true,
        _ => return Err("Redis 会话族撤销状态无效".into()),
    };
    if !present
        || actual != sid
        || tenant != subject.tenant
        || user != subject.user_id
        || !canonical_v4(jti, false)
        || rotated_at <= 0
        || absolute_exp <= rotated_at
        || expires != absolute_exp
    {
        return Err("Redis 会话族与已登记身份或过期绑定不同".into());
    }
    Ok(Some(Family {
        absolute_exp,
        rotated_at,
        revoked,
        current_jti_sha256: sha256(jti),
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    sid: String,
    tenant_id: String,
    user_id: i64,
    username: String,
    dept_name: Value,
    ipaddr: String,
    login_location: Value,
    browser: Value,
    os: Value,
    login_time: String,
    last_access_time: String,
    absolute_exp: i64,
}

fn read_metadata(
    read: &mut ReadCommand<'_>,
    key: &str,
    subject: &Subject<'_>,
    sid: &str,
) -> Result<Option<OnlineMetadata>> {
    let present = key_type(read, key, "string")?;
    let raw = read(&["GET", key])?;
    let expires = redis_integer(&read(&["EXPIRETIME", key])?)?;
    if !present && raw.is_null() && expires == -2 {
        return Ok(None);
    }
    let raw = raw.as_str().ok_or("Redis 在线元数据结构无效")?;
    let data: Metadata = serde_json::from_str(raw).map_err(|_| "Redis 在线元数据格式无效")?;
    let login = epoch(&data.login_time)?;
    let last_access = epoch(&data.last_access_time)?;
    if !present
        || data.sid != sid
        || data.tenant_id != subject.tenant
        || data.user_id != subject.user_id
        || data.username != subject.username
        || data.ipaddr != subject.client_address
        || login != subject.login_epoch
        || last_access < login
        || data.absolute_exp <= last_access
        || expires <= login
        || expires > data.absolute_exp.saturating_add(1)
        || [
            &data.dept_name,
            &data.login_location,
            &data.browser,
            &data.os,
        ]
        .iter()
        .any(|value| !value.is_null() && !value.is_string())
    {
        return Err("Redis 在线元数据与已登记原身份或有效期不同".into());
    }
    Ok(Some(OnlineMetadata {
        absolute_exp: data.absolute_exp,
        expires,
        last_access,
        sha256: sha256(raw),
    }))
}

fn report(
    snapshot: Snapshot,
    subject: &Subject<'_>,
    now: i64,
    require_active: bool,
) -> Result<Readback> {
    if snapshot
        .family
        .as_ref()
        .is_some_and(|family| family.rotated_at > now)
        || snapshot
            .metadata
            .as_ref()
            .is_some_and(|metadata| metadata.last_access > now)
    {
        return Err("会话记录时间晚于 Redis 核对时间".into());
    }
    let family = match &snapshot.family {
        None => "absent",
        Some(family) if family.revoked => "revoked",
        Some(family) if family.absolute_exp <= now => "exhausted",
        Some(_) => "active",
    };
    let metadata_present = snapshot.metadata.is_some();
    let active = family == "active"
        && snapshot
            .metadata
            .as_ref()
            .is_some_and(|metadata| metadata.expires > now)
        && snapshot.indexes.iter().all(|present| *present);
    let cleared =
        family != "active" && !metadata_present && snapshot.indexes.iter().all(|present| !*present);
    if require_active && !active {
        return Err("发现的原会话不具有完整在用前像，保留现场".into());
    }
    let [refresh_tenant, refresh_user, online_tenant, online_user] = snapshot.indexes;
    let proof = json!({"format_version":1,"kind":"devex-seed-identity-session-redis-readback",
        "scope_id":subject.scope,"sid_sha256":sha256(&snapshot.sid),"observed_at_unix_seconds":now,
        "family":family,"online_metadata_present":metadata_present,"indexes":{
            "refresh_tenant":refresh_tenant,"refresh_user":refresh_user,
            "online_tenant":online_tenant,"online_user":online_user},
        "active":active,"cleared":cleared});
    Ok(Readback {
        sid: snapshot.sid,
        proof,
        active,
        cleared,
    })
}

fn key_type(read: &mut ReadCommand<'_>, key: &str, expected: &str) -> Result<bool> {
    let actual = read(&["TYPE", key])?;
    match actual.as_str() {
        Some("none") => Ok(false),
        Some(actual) if actual == expected => Ok(true),
        _ => Err("Redis 会话键类型与原会话结构不同".into()),
    }
}

fn redis_integer(value: &Value) -> Result<i64> {
    let value = value.as_str().ok_or("Redis 会话命令返回了非整数响应")?;
    let number = value.parse::<i64>().map_err(|_| "Redis 会话整数响应无效")?;
    if number.to_string() != value {
        return Err("Redis 会话整数响应非规范形式".into());
    }
    Ok(number)
}

fn redis_time(read: &mut ReadCommand<'_>) -> Result<(i64, i64)> {
    let reply = read(&["TIME"])?;
    let Some([seconds, micros]) = reply.as_array().map(Vec::as_slice) else {
        return Err("Redis TIME 响应无效".into());
    };
    let (seconds, micros) = (redis_integer(seconds)?, redis_integer(micros)?);
    if seconds <= 0 || !(0..1_000_000).contains(&micros) {
        return Err("Redis TIME 返回了无效时间".into());
    }
    Ok((seconds, micros))
}

fn epoch(value: &str) -> Result<i64> {
    let value = OffsetDateTime::parse(value, &Rfc3339).map_err(|_| "在线会话 UTC 时间无效")?;
    if !value.offset().is_utc() {
        return Err("在线会话 UTC 偏移无效".into());
    }
    Ok(value.unix_timestamp())
}

fn require_sid(subject: &Subject<'_>, sid: &str) -> Result<()> {
    if !sid
        .strip_prefix("s-")
        .is_some_and(|value| canonical_v4(value, true))
        || !sha256(sid).eq_ignore_ascii_case(subject.sid_sha256)
    {
        return Err("Redis 会话标识与预登记摘要不匹配，保留现场".into());
    }
    Ok(())
}

fn canonical_v4(value: &str, hyphenated: bool) -> bool {
    Uuid::parse_str(value).is_ok_and(|uuid| {
        uuid.get_version_num() == 4
            && uuid.get_variant() == Variant::RFC4122
            && if hyphenated {
                uuid.hyphenated().to_string() == value
            } else {
                uuid.simple().to_string() == value
            }
    })
}

fn sha256(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/post_process/identity_session_redis.rs"]
mod tests;
