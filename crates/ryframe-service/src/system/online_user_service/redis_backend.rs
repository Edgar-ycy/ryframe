use chrono::Utc;
use ryframe_common::{AppError, AppResult};
use ryframe_core::RedisClient;

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
        // MGET preserves key order; keys expiring after SCAN are returned as None.
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

#[cfg(test)]
mod tests {
    use ryframe_config::{RedisConfig, RedisMode};
    use uuid::Uuid;

    use super::{
        MGET_BATCH_SIZE, TouchCasOutcome, apply_touch_if_unchanged, parse_touch_cas_outcome,
    };
    use ryframe_core::RedisClient;

    #[test]
    fn mget_batches_are_bounded() {
        let keys = (0..(MGET_BATCH_SIZE * 2 + 1)).collect::<Vec<_>>();
        assert_eq!(
            keys.chunks(MGET_BATCH_SIZE)
                .map(<[usize]>::len)
                .collect::<Vec<_>>(),
            [MGET_BATCH_SIZE, MGET_BATCH_SIZE, 1]
        );
    }

    #[test]
    fn touch_cas_status_codes_are_explicit_and_fail_closed() {
        assert_eq!(
            parse_touch_cas_outcome(0).unwrap(),
            TouchCasOutcome::Skipped
        );
        assert_eq!(
            parse_touch_cas_outcome(1).unwrap(),
            TouchCasOutcome::Updated
        );
        assert_eq!(
            parse_touch_cas_outcome(2).unwrap(),
            TouchCasOutcome::Deleted
        );
        assert!(parse_touch_cas_outcome(3).is_err());
    }

    async fn docker_redis() -> RedisClient {
        let port = std::env::var("RYFRAME_TEST_REDIS_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(16379);
        RedisClient::connect(&RedisConfig {
            mode: RedisMode::Required,
            host: "127.0.0.1".into(),
            port,
            timeout_secs: 2,
            ..RedisConfig::default()
        })
        .await
        .expect(
            "connect Redis test service; run `docker compose -f docker-compose.test.yml up -d --wait`",
        )
    }

    #[tokio::test]
    #[ignore = "requires Docker Compose Redis service"]
    async fn stale_touch_cannot_resurrect_or_overwrite_online_user_index() {
        let client = docker_redis().await;
        let key = format!("ryframe:test:online-user-touch-cas:{}", Uuid::new_v4());
        let original = r#"{"version":"original"}"#;
        let replacement = r#"{"version":"replacement"}"#;
        let touched = r#"{"version":"touched"}"#;

        client.set_ex(&key, original, 60).await.unwrap();
        let expected = client.get(&key).await.unwrap().unwrap();
        client.del(&key).await.unwrap();
        assert_eq!(
            apply_touch_if_unchanged(&client, &key, &expected, Some((touched, 60)))
                .await
                .unwrap(),
            TouchCasOutcome::Skipped
        );
        assert_eq!(client.get(&key).await.unwrap(), None);

        client.set_ex(&key, original, 60).await.unwrap();
        let expected = client.get(&key).await.unwrap().unwrap();
        client.set_ex(&key, replacement, 60).await.unwrap();
        assert_eq!(
            apply_touch_if_unchanged(&client, &key, &expected, Some((touched, 60)))
                .await
                .unwrap(),
            TouchCasOutcome::Skipped
        );
        assert_eq!(
            client.get(&key).await.unwrap().as_deref(),
            Some(replacement)
        );

        client.set_ex(&key, original, 60).await.unwrap();
        let expected = client.get(&key).await.unwrap().unwrap();
        assert_eq!(
            apply_touch_if_unchanged(&client, &key, &expected, Some((touched, 60)))
                .await
                .unwrap(),
            TouchCasOutcome::Updated
        );
        assert_eq!(client.get(&key).await.unwrap().as_deref(), Some(touched));

        let expected = client.get(&key).await.unwrap().unwrap();
        assert_eq!(
            apply_touch_if_unchanged(&client, &key, &expected, None)
                .await
                .unwrap(),
            TouchCasOutcome::Deleted
        );
        assert_eq!(client.get(&key).await.unwrap(), None);
    }
}
