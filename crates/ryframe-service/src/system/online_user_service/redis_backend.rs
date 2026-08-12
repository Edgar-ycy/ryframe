use chrono::Utc;
use ryframe_core::RedisClient;
use ryframe_kernel::{AppError, AppResult};

use super::{
    UserSession,
    keyspace::{session_key, tenant_index_key, tenant_pattern, tenant_user_index_key},
    session_codec::{decode_batch, encode, remaining_ttl},
};

const MGET_BATCH_SIZE: usize = 256;

const READ_INDEX_SCRIPT: &str = r#"
local result = redis.call('SMEMBERS', KEYS[1])
table.sort(result)
return result
"#;

const ADD_SCRIPT: &str = r#"
redis.call('SETEX', KEYS[1], tonumber(ARGV[2]), ARGV[1])
redis.call('SADD', KEYS[2], ARGV[3])
redis.call('SADD', KEYS[3], ARGV[3])
local tenant_ttl = redis.call('TTL', KEYS[2])
local user_ttl = redis.call('TTL', KEYS[3])
if tenant_ttl == -1 or tenant_ttl < tonumber(ARGV[2]) then
  redis.call('EXPIRE', KEYS[2], tonumber(ARGV[2]))
end
if user_ttl == -1 or user_ttl < tonumber(ARGV[2]) then
  redis.call('EXPIRE', KEYS[3], tonumber(ARGV[2]))
end
return 1
"#;

const REMOVE_SCRIPT: &str = r#"
redis.call('DEL', KEYS[1])
redis.call('SREM', KEYS[2], ARGV[1])
if ARGV[2] == '1' then
  redis.call('SREM', KEYS[3], ARGV[1])
  if redis.call('SCARD', KEYS[3]) == 0 then redis.call('DEL', KEYS[3]) end
end
if redis.call('SCARD', KEYS[2]) == 0 then redis.call('DEL', KEYS[2]) end
return 1
"#;

const TOUCH_IF_UNCHANGED_SCRIPT: &str = r#"
local current = redis.call('GET', KEYS[1])
if current == false or current ~= ARGV[1] then return 0 end
local ttl = tonumber(ARGV[3])
if ttl == nil or ttl <= 0 then
  redis.call('DEL', KEYS[1])
  redis.call('SREM', KEYS[2], ARGV[4])
  redis.call('SREM', KEYS[3], ARGV[4])
  return 2
end
redis.call('SETEX', KEYS[1], ttl, ARGV[2])
redis.call('SADD', KEYS[2], ARGV[4])
redis.call('SADD', KEYS[3], ARGV[4])
local tenant_ttl = redis.call('TTL', KEYS[2])
local user_ttl = redis.call('TTL', KEYS[3])
if tenant_ttl == -1 or tenant_ttl < ttl then redis.call('EXPIRE', KEYS[2], ttl) end
if user_ttl == -1 or user_ttl < ttl then redis.call('EXPIRE', KEYS[3], ttl) end
return 1
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TouchCasOutcome {
    Skipped,
    Updated,
    Deleted,
}

fn unavailable(operation: &str, error: redis::RedisError) -> AppError {
    tracing::error!(%error, operation, "Redis 登录设备操作失败");
    AppError::ServiceUnavailable("登录设备服务暂不可用".into())
}

fn parse_touch_cas_outcome(code: i64) -> AppResult<TouchCasOutcome> {
    match code {
        0 => Ok(TouchCasOutcome::Skipped),
        1 => Ok(TouchCasOutcome::Updated),
        2 => Ok(TouchCasOutcome::Deleted),
        code => {
            tracing::error!(code, "Redis 在线用户 CAS 返回未知状态");
            Err(AppError::ServiceUnavailable("登录设备服务暂不可用".into()))
        }
    }
}

async fn apply_touch_if_unchanged(
    client: &RedisClient,
    session: &UserSession,
    expected_json: &str,
    replacement: Option<(&str, u64)>,
) -> AppResult<TouchCasOutcome> {
    let metadata_key = session_key(&session.tenant_id, &session.sid);
    let tenant_key = tenant_index_key(&session.tenant_id);
    let user_key = tenant_user_index_key(&session.tenant_id, session.user_id);
    let (new_json, ttl) = replacement
        .map(|(json, ttl)| (json, ttl.to_string()))
        .unwrap_or(("", "0".to_string()));
    let code = client
        .eval_script_i64(
            TOUCH_IF_UNCHANGED_SCRIPT,
            &[
                metadata_key.as_str(),
                tenant_key.as_str(),
                user_key.as_str(),
            ],
            &[expected_json, new_json, ttl.as_str(), session.sid.as_str()],
        )
        .await
        .map_err(|error| unavailable("touch", error))?;
    parse_touch_cas_outcome(code)
}

pub(super) async fn add(client: &RedisClient, session: &UserSession, ttl: u64) -> AppResult<()> {
    let metadata_key = session_key(&session.tenant_id, &session.sid);
    let tenant_key = tenant_index_key(&session.tenant_id);
    let user_key = tenant_user_index_key(&session.tenant_id, session.user_id);
    let json = encode(session).map_err(|error| {
        tracing::error!(%error, "序列化在线用户失败");
        AppError::Internal("无法序列化登录设备元数据".into())
    })?;
    let ttl = ttl.to_string();
    client
        .eval_script_i64(
            ADD_SCRIPT,
            &[
                metadata_key.as_str(),
                tenant_key.as_str(),
                user_key.as_str(),
            ],
            &[json.as_str(), ttl.as_str(), session.sid.as_str()],
        )
        .await
        .map_err(|error| unavailable("add", error))?;
    Ok(())
}

pub(super) async fn remove(client: &RedisClient, tenant_id: &str, sid: &str) -> AppResult<()> {
    let metadata_key = session_key(tenant_id, sid);
    let tenant_key = tenant_index_key(tenant_id);
    // 在 Rust 中解析 Snowflake ID，避免 Lua cjson 以双精度数处理时丢失精度。
    let metadata = client
        .get(&metadata_key)
        .await
        .map_err(|error| unavailable("remove_get", error))?;
    let user_id = metadata.as_deref().and_then(|json| {
        serde_json::from_str::<UserSession>(json)
            .inspect_err(|error| {
                tracing::warn!(%error, sid, "清理登录设备时无法解析用户 ID");
            })
            .ok()
            .filter(|session| session.tenant_id == tenant_id && session.sid == sid)
            .map(|session| session.user_id)
    });
    let user_key = user_id
        .map(|user_id| tenant_user_index_key(tenant_id, user_id))
        .unwrap_or_else(|| tenant_key.clone());
    client
        .eval_script_i64(
            REMOVE_SCRIPT,
            &[
                metadata_key.as_str(),
                tenant_key.as_str(),
                user_key.as_str(),
            ],
            &[sid, if user_id.is_some() { "1" } else { "0" }],
        )
        .await
        .map_err(|error| unavailable("remove", error))?;
    Ok(())
}

async fn load_keys(
    client: &RedisClient,
    tenant_id: &str,
    keys: Vec<String>,
) -> AppResult<Vec<UserSession>> {
    let mut sessions = Vec::with_capacity(keys.len());
    for key_batch in keys.chunks(MGET_BATCH_SIZE) {
        let values = client
            .mget(key_batch)
            .await
            .map_err(|error| unavailable("list", error))?;
        sessions.extend(decode_batch(tenant_id, key_batch, values)?);
    }
    Ok(sessions)
}

/// 同时读取旧版 metadata SCAN 与新版租户索引，兼容升级前最多七天的会话。
pub(super) async fn list(client: &RedisClient, tenant_id: &str) -> AppResult<Vec<UserSession>> {
    let mut keys = client
        .scan_keys(tenant_pattern(tenant_id))
        .await
        .map_err(|error| unavailable("legacy_list", error))?;
    let indexed_sids = client
        .eval_script_optional_strings(
            READ_INDEX_SCRIPT,
            &[tenant_index_key(tenant_id).as_str()],
            &[] as &[&str],
        )
        .await
        .map_err(|error| unavailable("tenant_index", error))?;
    keys.extend(
        indexed_sids
            .into_iter()
            .flatten()
            .map(|sid| session_key(tenant_id, &sid)),
    );
    keys.sort_unstable();
    keys.dedup();
    load_keys(client, tenant_id, keys).await
}

pub(super) async fn list_for_user(
    client: &RedisClient,
    tenant_id: &str,
    user_id: i64,
) -> AppResult<Vec<UserSession>> {
    // 首版双读不能只依赖新索引：升级前的会话只有旧 metadata key。
    let mut keys = client
        .scan_keys(tenant_pattern(tenant_id))
        .await
        .map_err(|error| unavailable("legacy_user_list", error))?;
    let indexed_sids = client
        .eval_script_optional_strings(
            READ_INDEX_SCRIPT,
            &[tenant_user_index_key(tenant_id, user_id).as_str()],
            &[] as &[&str],
        )
        .await
        .map_err(|error| unavailable("user_index", error))?;
    keys.extend(
        indexed_sids
            .into_iter()
            .flatten()
            .map(|sid| session_key(tenant_id, &sid)),
    );
    keys.sort_unstable();
    keys.dedup();
    let mut sessions = load_keys(client, tenant_id, keys).await?;
    sessions.retain(|session| session.user_id == user_id);
    Ok(sessions)
}

pub(super) async fn touch(client: &RedisClient, tenant_id: &str, sid: &str) -> AppResult<()> {
    let key = session_key(tenant_id, sid);
    let Some(json) = client
        .get(&key)
        .await
        .map_err(|error| unavailable("touch_get", error))?
    else {
        return Err(AppError::ServiceUnavailable(
            "登录设备元数据暂不可用".into(),
        ));
    };
    let mut session = serde_json::from_str::<UserSession>(&json).map_err(|error| {
        tracing::error!(%error, %key, "反序列化在线用户失败");
        AppError::ServiceUnavailable("登录设备元数据损坏".into())
    })?;
    if session.tenant_id != tenant_id || session.sid != sid {
        tracing::warn!(%key, "拒绝更新身份不匹配的在线用户元数据");
        return Ok(());
    }
    session.last_access_time = Utc::now();
    let replacement = encode(&session)
        .ok()
        .zip(remaining_ttl(session.absolute_exp));
    let replacement_ref = replacement
        .as_ref()
        .map(|(new_json, ttl)| (new_json.as_str(), *ttl));
    match apply_touch_if_unchanged(client, &session, &json, replacement_ref).await? {
        TouchCasOutcome::Updated => Ok(()),
        TouchCasOutcome::Skipped | TouchCasOutcome::Deleted => Err(AppError::ServiceUnavailable(
            "登录设备元数据暂不可用".into(),
        )),
    }
}
