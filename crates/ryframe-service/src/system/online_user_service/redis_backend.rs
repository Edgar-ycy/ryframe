use chrono::Utc;
use redis::AsyncCommands;
use ryframe_core::RedisClient;
use ryframe_kernel::{AppError, AppResult};

use super::{
    UserSession,
    keyspace::{session_key, tenant_index_key, tenant_pattern, tenant_user_index_key},
    session_codec::{decode_batch, encode, remaining_ttl},
};

const MGET_BATCH_SIZE: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TouchCasOutcome {
    Skipped,
    Updated,
    Deleted,
}

struct SessionIndexMembership<'a> {
    metadata_key: &'a str,
    tenant_key: &'a str,
    user_key: &'a str,
    sid: &'a str,
    json: &'a str,
    ttl_secs: u64,
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
    let watched = [metadata_key.clone(), tenant_key.clone(), user_key.clone()];
    let expected_json = expected_json.to_owned();
    let replacement = replacement.map(|(json, ttl)| (json.to_owned(), ttl));
    let sid = session.sid.clone();
    let code = client
        .transaction(&watched, move |mut connection, mut transaction| {
            let metadata_key = metadata_key.clone();
            let tenant_key = tenant_key.clone();
            let user_key = user_key.clone();
            let expected_json = expected_json.clone();
            let replacement = replacement.clone();
            let sid = sid.clone();
            async move {
                let current: Option<String> = connection.get(&metadata_key).await?;
                if current.as_deref() != Some(expected_json.as_str()) {
                    return Ok(Some(0_i64));
                }
                let Some((new_json, ttl)) = replacement else {
                    transaction
                        .del(&metadata_key)
                        .ignore()
                        .srem(&tenant_key, &sid)
                        .ignore()
                        .srem(&user_key, &sid)
                        .ignore();
                    let committed: Option<()> = transaction.query_async(&mut connection).await?;
                    return Ok(committed.map(|()| 2_i64));
                };
                let membership = SessionIndexMembership {
                    metadata_key: &metadata_key,
                    tenant_key: &tenant_key,
                    user_key: &user_key,
                    sid: &sid,
                    json: &new_json,
                    ttl_secs: ttl,
                };
                queue_index_membership(&mut connection, &mut transaction, &membership).await?;
                let committed: Option<()> = transaction.query_async(&mut connection).await?;
                Ok(committed.map(|()| 1_i64))
            }
        })
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
    let watched = [metadata_key.clone(), tenant_key.clone(), user_key.clone()];
    let sid = session.sid.clone();
    let _: () = client
        .transaction(&watched, move |mut connection, mut transaction| {
            let metadata_key = metadata_key.clone();
            let tenant_key = tenant_key.clone();
            let user_key = user_key.clone();
            let json = json.clone();
            let sid = sid.clone();
            async move {
                let membership = SessionIndexMembership {
                    metadata_key: &metadata_key,
                    tenant_key: &tenant_key,
                    user_key: &user_key,
                    sid: &sid,
                    json: &json,
                    ttl_secs: ttl,
                };
                queue_index_membership(&mut connection, &mut transaction, &membership).await?;
                transaction.query_async(&mut connection).await
            }
        })
        .await
        .map_err(|error| unavailable("add", error))?;
    Ok(())
}

pub(super) async fn remove(client: &RedisClient, tenant_id: &str, sid: &str) -> AppResult<()> {
    let metadata_key = session_key(tenant_id, sid);
    let tenant_key = tenant_index_key(tenant_id);
    // 在 Rust 中解析 Snowflake ID，避免经浮点转换时丢失精度。
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
    let watched = [metadata_key.clone(), tenant_key.clone(), user_key.clone()];
    let sid = sid.to_owned();
    client
        .transaction(&watched, move |mut connection, mut transaction| {
            let metadata_key = metadata_key.clone();
            let tenant_key = tenant_key.clone();
            let user_key = user_key.clone();
            let sid = sid.clone();
            async move {
                transaction
                    .del(&metadata_key)
                    .ignore()
                    .srem(&tenant_key, &sid)
                    .ignore();
                if user_id.is_some() {
                    transaction.srem(&user_key, &sid).ignore();
                }
                let committed: Option<()> = transaction.query_async(&mut connection).await?;
                Ok(committed)
            }
        })
        .await
        .map_err(|error| unavailable("remove", error))?;
    Ok(())
}

async fn queue_index_membership(
    connection: &mut redis::aio::MultiplexedConnection,
    transaction: &mut redis::Pipeline,
    membership: &SessionIndexMembership<'_>,
) -> Result<(), redis::RedisError> {
    transaction
        .set_ex(
            membership.metadata_key,
            membership.json,
            membership.ttl_secs,
        )
        .ignore()
        .sadd(membership.tenant_key, membership.sid)
        .ignore()
        .sadd(membership.user_key, membership.sid)
        .ignore();
    let ttl_secs = redis_ttl_secs(membership.ttl_secs);
    let tenant_ttl: i64 = connection.ttl(membership.tenant_key).await?;
    let user_ttl: i64 = connection.ttl(membership.user_key).await?;
    if tenant_ttl == -1 || tenant_ttl < ttl_secs {
        transaction.expire(membership.tenant_key, ttl_secs).ignore();
    }
    if user_ttl == -1 || user_ttl < ttl_secs {
        transaction.expire(membership.user_key, ttl_secs).ignore();
    }
    Ok(())
}

fn redis_ttl_secs(ttl_secs: u64) -> i64 {
    ttl_secs.min(i64::MAX as u64) as i64
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
    let indexed_sids: Vec<String> = client
        .conn()
        .clone()
        .smembers(tenant_index_key(tenant_id))
        .await
        .map_err(|error| unavailable("tenant_index", error))?;
    keys.extend(
        indexed_sids
            .into_iter()
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
    let indexed_sids: Vec<String> = client
        .conn()
        .clone()
        .smembers(tenant_user_index_key(tenant_id, user_id))
        .await
        .map_err(|error| unavailable("user_index", error))?;
    keys.extend(
        indexed_sids
            .into_iter()
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
