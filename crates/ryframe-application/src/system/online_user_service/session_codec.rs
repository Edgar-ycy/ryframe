use chrono::Utc;
use ryframe_adapters::RedisNamespace;
use ryframe_kernel::{AppError, AppResult};

use super::{UserSession, keyspace::session_key};

pub(super) fn remaining_ttl(absolute_exp: i64) -> Option<u64> {
    let remaining = absolute_exp - Utc::now().timestamp();
    (remaining > 0).then_some(remaining as u64)
}

pub(super) fn encode(session: &UserSession) -> serde_json::Result<String> {
    serde_json::to_string(session)
}

pub(super) fn decode_batch(
    scope: &RedisNamespace,
    expected_tenant_id: &str,
    keys: &[String],
    values: Vec<Option<String>>,
) -> AppResult<Vec<UserSession>> {
    if keys.len() != values.len() {
        tracing::error!(
            key_count = keys.len(),
            value_count = values.len(),
            "Redis MGET 在线用户返回数量异常"
        );
        return Err(AppError::ServiceUnavailable("登录设备服务暂不可用".into()));
    }

    let mut sessions = Vec::with_capacity(keys.len());
    for (key, value) in keys.iter().zip(values) {
        let Some(json) = value else {
            continue;
        };
        let session = serde_json::from_str::<UserSession>(&json).map_err(|error| {
            tracing::error!(%error, %key, "反序列化在线用户失败");
            AppError::ServiceUnavailable("登录设备元数据损坏".into())
        })?;
        if session.tenant_id != expected_tenant_id
            || key != &scope.key(&session_key(expected_tenant_id, &session.sid))
        {
            tracing::warn!(
                %key,
                expected_tenant_id,
                session_tenant_id = session.tenant_id,
                "忽略请求租户之外的在线用户索引"
            );
            continue;
        }
        if remaining_ttl(session.absolute_exp).is_some() {
            sessions.push(session);
        }
    }
    Ok(sessions)
}
