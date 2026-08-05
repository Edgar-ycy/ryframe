use chrono::Utc;
use ryframe_core::RedisClient;
use ryframe_kernel::{AppError, AppResult};

use super::{
    OnlineUserVo, UserSession,
    keyspace::{session_key, tenant_pattern},
    session_codec::{decode_batch, encode, remaining_ttl},
};

const MGET_BATCH_SIZE: usize = 256;
const TOUCH_IF_UNCHANGED_SCRIPT: &str = r#"
local current = redis.call('GET', KEYS[1])
if current == false or current ~= ARGV[1] then
    return 0
end
local ttl = tonumber(ARGV[3])
if ttl == nil or ttl <= 0 then
    redis.call('DEL', KEYS[1])
    return 2
end
redis.call('SETEX', KEYS[1], ttl, ARGV[2])
return 1
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TouchCasOutcome {
    Skipped,
    Updated,
    Deleted,
}

fn parse_touch_cas_outcome(code: i64) -> Result<TouchCasOutcome, String> {
    match code {
        0 => Ok(TouchCasOutcome::Skipped),
        1 => Ok(TouchCasOutcome::Updated),
        2 => Ok(TouchCasOutcome::Deleted),
        code => Err(format!("Redis 在线用户 CAS 返回未知状态: {code}")),
    }
}

async fn apply_touch_if_unchanged(
    client: &RedisClient,
    key: &str,
    expected_json: &str,
    replacement: Option<(&str, u64)>,
) -> Result<TouchCasOutcome, String> {
    let (new_json, ttl) = replacement
        .map(|(json, ttl)| (json, ttl.to_string()))
        .unwrap_or(("", "0".to_string()));
    let code = client
        .eval_script_i64(
            TOUCH_IF_UNCHANGED_SCRIPT,
            &[key],
            &[expected_json, new_json, ttl.as_str()],
        )
        .await
        .map_err(|error| format!("Redis 在线用户 CAS 续期失败: {error}"))?;
    parse_touch_cas_outcome(code)
}

pub(super) async fn add(client: &RedisClient, session: &UserSession, ttl: u64) {
    let key = session_key(&session.tenant_id, &session.sid);
    let json = match encode(session) {
        Ok(json) => json,
        Err(error) => {
            tracing::error!("序列化在线用户失败: {}", error);
            return;
        }
    };
    if let Err(error) = client.set_ex(&key, &json, ttl).await {
        tracing::error!("Redis SET 在线用户失败: {}", error);
    }
}

pub(super) async fn remove(client: &RedisClient, tenant_id: &str, sid: &str) {
    let key = session_key(tenant_id, sid);
    if let Err(error) = client.del(&key).await {
        tracing::error!("Redis DEL 在线用户失败: {}", error);
    }
}

pub(super) async fn list(client: &RedisClient, tenant_id: &str) -> AppResult<Vec<OnlineUserVo>> {
    let pattern = tenant_pattern(tenant_id);
    let keys = client.scan_keys(&pattern).await.map_err(|error| {
        tracing::error!("Redis SCAN 在线用户失败: {}", error);
        AppError::Internal("查询在线用户失败".into())
    })?;
    let mut users = Vec::with_capacity(keys.len());
    for key_batch in keys.chunks(MGET_BATCH_SIZE) {
        // MGET 保持键顺序；在 SCAN 之后过期的键会以 None 返回。
        let values = client.mget(key_batch).await.map_err(|error| {
            tracing::error!("Redis MGET 在线用户失败: {}", error);
            AppError::Internal("查询在线用户失败".into())
        })?;
        users.extend(decode_batch(tenant_id, key_batch, values)?);
    }
    Ok(users)
}

pub(super) async fn touch(client: &RedisClient, tenant_id: &str, sid: &str) {
    let key = session_key(tenant_id, sid);
    match client.get(&key).await {
        Ok(Some(json)) => {
            if let Ok(mut session) = serde_json::from_str::<UserSession>(&json) {
                session.last_access_time = Utc::now();
                let replacement = encode(&session)
                    .ok()
                    .zip(remaining_ttl(session.absolute_exp));
                let replacement_ref = replacement
                    .as_ref()
                    .map(|(new_json, ttl)| (new_json.as_str(), *ttl));
                match apply_touch_if_unchanged(client, &key, &json, replacement_ref).await {
                    Ok(TouchCasOutcome::Skipped) => {
                        tracing::debug!("在线用户索引已被删除或更新，跳过过期 touch");
                    }
                    Ok(TouchCasOutcome::Updated | TouchCasOutcome::Deleted) => {}
                    Err(error) => {
                        tracing::warn!(%error, "Redis 在线用户 touch 失败");
                    }
                }
            }
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!("Redis GET touch_user 失败: {}", error);
        }
    }
}
