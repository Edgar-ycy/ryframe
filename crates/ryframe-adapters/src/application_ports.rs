use std::sync::Arc;

use async_trait::async_trait;
use ryframe_application::system::{
    content::DictCacheStore,
    identity::{CaptchaStore, WebSocketTicketStore},
    platform::{TenantRateLimitReadPort, TenantRateLimitSnapshot},
};
use ryframe_kernel::{AppError, CAPTCHA_KEY_PREFIX};

use crate::{RedisClient, rate_limit::RateLimiter};

struct RedisDictCacheStore {
    client: RedisClient,
}

struct RedisCaptchaStore {
    client: RedisClient,
    ttl_secs: u64,
}

struct RedisWebSocketTicketStore {
    client: RedisClient,
}

struct TenantRateLimitReader {
    limiter: Arc<RateLimiter>,
}

/// 构造 Redis 字典缓存端口。
pub fn redis_dict_cache_store(client: RedisClient) -> Arc<dyn DictCacheStore> {
    Arc::new(RedisDictCacheStore { client })
}

/// 构造 Redis 验证码端口。
pub fn redis_captcha_store(client: RedisClient, ttl_secs: u64) -> Arc<dyn CaptchaStore> {
    Arc::new(RedisCaptchaStore { client, ttl_secs })
}

/// 构造 Redis WebSocket 一次性票据端口。
pub fn redis_websocket_ticket_store(client: RedisClient) -> Arc<dyn WebSocketTicketStore> {
    Arc::new(RedisWebSocketTicketStore { client })
}

/// 构造租户限流快照读取端口。
pub fn tenant_rate_limit_reader(limiter: Arc<RateLimiter>) -> Arc<dyn TenantRateLimitReadPort> {
    Arc::new(TenantRateLimitReader { limiter })
}

#[async_trait]
impl DictCacheStore for RedisDictCacheStore {
    async fn get(&self, key: &str) -> Result<Option<String>, AppError> {
        self.client
            .get(key)
            .await
            .map_err(|error| AppError::ServiceUnavailable(error.to_string()))
    }

    async fn put(&self, key: String, value: String, ttl_secs: u64) -> Result<(), AppError> {
        self.client
            .set_ex(key, value, ttl_secs)
            .await
            .map_err(|error| AppError::ServiceUnavailable(error.to_string()))
    }

    async fn remove(&self, key: String) -> Result<(), AppError> {
        self.client
            .del(key)
            .await
            .map(|_| ())
            .map_err(|error| AppError::ServiceUnavailable(error.to_string()))
    }
}

#[async_trait]
impl CaptchaStore for RedisCaptchaStore {
    async fn set(&self, id: String, answer: String) -> Result<(), AppError> {
        let key = format!("{CAPTCHA_KEY_PREFIX}{id}");
        self.client
            .set_ex(key, answer, self.ttl_secs)
            .await
            .map_err(|error| {
                tracing::error!(%error, "Redis SET 验证码失败");
                AppError::ServiceUnavailable("验证码服务暂不可用".into())
            })
    }

    async fn verify(&self, id: &str, code: &str) -> Result<bool, AppError> {
        let key = format!("{CAPTCHA_KEY_PREFIX}{id}");
        self.client
            .get_and_del(&key)
            .await
            .map(|stored| stored.is_some_and(|value| value.eq_ignore_ascii_case(code)))
            .map_err(|error| {
                tracing::error!(%error, "Redis GETDEL 验证码失败");
                AppError::ServiceUnavailable("验证码服务暂不可用".into())
            })
    }
}

#[async_trait]
impl WebSocketTicketStore for RedisWebSocketTicketStore {
    async fn put(&self, key: String, value: String, ttl_secs: u64) -> Result<(), AppError> {
        self.client
            .set_ex(key, value, ttl_secs)
            .await
            .map_err(|error| {
                AppError::ServiceUnavailable(format!("WebSocket 票据写入失败: {error}"))
            })
    }

    async fn take(&self, key: &str) -> Result<Option<String>, AppError> {
        self.client.get_and_del(key).await.map_err(|error| {
            AppError::ServiceUnavailable(format!("WebSocket 票据校验失败: {error}"))
        })
    }
}

#[async_trait]
impl TenantRateLimitReadPort for TenantRateLimitReader {
    async fn snapshot_many(
        &self,
        tenant_ids: &[String],
    ) -> Result<Vec<TenantRateLimitSnapshot>, AppError> {
        let keys = tenant_ids
            .iter()
            .map(|tenant_id| RateLimiter::tenant_key(tenant_id))
            .collect::<Vec<_>>();
        self.limiter
            .snapshot_many(&keys, 1)
            .await
            .map_err(AppError::ServiceUnavailable)
            .map(|snapshots| {
                snapshots
                    .into_iter()
                    .map(|snapshot| TenantRateLimitSnapshot {
                        current: snapshot.current,
                        remaining_secs: snapshot.remaining_secs,
                    })
                    .collect()
            })
    }
}
