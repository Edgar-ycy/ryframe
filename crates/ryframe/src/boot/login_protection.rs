use std::sync::Arc;

use ryframe_adapters::RedisClient;
use ryframe_application::{LoginProtectionFuture, LoginProtectionPort};
use ryframe_kernel::{AppError, AppResult};

struct RedisLoginProtection {
    redis: Option<RedisClient>,
}

impl LoginProtectionPort for RedisLoginProtection {
    fn ensure_allowed<'a>(
        &'a self,
        tenant_id: &'a str,
        username: &'a str,
        ip: &'a str,
        max_attempts: u32,
    ) -> LoginProtectionFuture<'a> {
        Box::pin(async move {
            let Some(redis) = self.redis.as_ref() else {
                return Ok(());
            };
            check_counter(
                redis,
                &principal_key(tenant_id, username),
                max_attempts,
                "账户",
            )
            .await?;
            check_counter(
                redis,
                &ip_key(tenant_id, ip),
                max_attempts.saturating_mul(2),
                "IP",
            )
            .await
        })
    }

    fn record_failure<'a>(
        &'a self,
        tenant_id: &'a str,
        username: &'a str,
        ip: &'a str,
        lockout_seconds: u64,
    ) -> LoginProtectionFuture<'a> {
        Box::pin(async move {
            let Some(redis) = self.redis.as_ref() else {
                return Ok(());
            };
            increment(redis, &principal_key(tenant_id, username), lockout_seconds).await?;
            increment(redis, &ip_key(tenant_id, ip), lockout_seconds).await
        })
    }

    fn clear<'a>(
        &'a self,
        tenant_id: &'a str,
        username: &'a str,
        ip: &'a str,
    ) -> LoginProtectionFuture<'a> {
        Box::pin(async move {
            let Some(redis) = self.redis.as_ref() else {
                return Ok(());
            };
            redis
                .del(&principal_key(tenant_id, username))
                .await
                .map_err(redis_unavailable)?;
            redis
                .del(&ip_key(tenant_id, ip))
                .await
                .map_err(redis_unavailable)?;
            Ok(())
        })
    }
}

pub fn store(redis: Option<RedisClient>) -> Arc<dyn LoginProtectionPort> {
    Arc::new(RedisLoginProtection { redis })
}

async fn check_counter(redis: &RedisClient, key: &str, limit: u32, subject: &str) -> AppResult<()> {
    let Some(count) = redis.get(key).await.map_err(redis_unavailable)? else {
        return Ok(());
    };
    let count = count
        .parse::<u32>()
        .map_err(|_| AppError::ServiceUnavailable("登录保护状态无效".into()))?;
    if count < limit {
        return Ok(());
    }
    let ttl = redis.ttl(key).await.map_err(redis_unavailable)?;
    if ttl <= 0 {
        return Ok(());
    }
    Err(AppError::Authentication(format!(
        "{subject}已被临时限制，请 {ttl} 秒后再试"
    )))
}

async fn increment(redis: &RedisClient, key: &str, ttl_seconds: u64) -> AppResult<()> {
    redis.incr(key).await.map_err(redis_unavailable)?;
    redis
        .expire(key, ttl_seconds)
        .await
        .map(|_| ())
        .map_err(redis_unavailable)
}

fn redis_unavailable(error: impl std::fmt::Display) -> AppError {
    tracing::error!(%error, "登录保护 Redis 操作失败");
    AppError::ServiceUnavailable("登录保护服务暂不可用".into())
}

fn principal_key(tenant_id: &str, username: &str) -> String {
    let normalized_username = username.trim().to_lowercase();
    let digest = ryframe_auth::stable_scope_digest(&[tenant_id, &normalized_username]);
    format!("ryframe:v0.5:login_fail:principal:{digest}")
}

fn ip_key(tenant_id: &str, ip: &str) -> String {
    let digest = ryframe_auth::stable_scope_digest(&[tenant_id, ip.trim()]);
    format!("ryframe:v0.5:login_fail:ip:{digest}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_normalize_principal_and_hide_raw_values() {
        assert_eq!(
            principal_key("tenant-a", " Alice "),
            principal_key("tenant-a", "alice")
        );
        assert!(!ip_key("tenant-a", "192.0.2.1").contains("192.0.2.1"));
    }

    #[tokio::test]
    async fn missing_redis_disables_login_protection_without_error() {
        let store = store(None);
        store
            .ensure_allowed("tenant-a", "alice", "192.0.2.1", 5)
            .await
            .expect("未配置 Redis 时不应阻止登录");
        store
            .record_failure("tenant-a", "alice", "192.0.2.1", 60)
            .await
            .expect("未配置 Redis 时记录失败应为空操作");
        store
            .clear("tenant-a", "alice", "192.0.2.1")
            .await
            .expect("未配置 Redis 时清理应为空操作");
    }
}
