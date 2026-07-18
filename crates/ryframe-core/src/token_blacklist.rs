//! Token 黑名单
//!
//! 用于实现 JWT 主动撤销：
//! - Redis 模式：存储 jti → "1"，设置 TTL = token 剩余有效时间
//! - 内存模式（仅在启动时显式选择 optional/disabled）：DashMap 存储，后台 GC 清理过期

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use dashmap::DashMap;

use crate::RedisClient;
use ryframe_common::{AppError, AppResult};

/// Token 黑名单
///
/// 用于在登出时主动撤销 JWT，防止令牌在有效期内被滥用。
/// 运行中的 Redis 错误会失败关闭；是否使用内存模式只在启动阶段决定。
#[derive(Clone)]
pub struct TokenBlacklist {
    redis: Option<RedisClient>,
    /// 内存模式：jti → 过期时刻
    local: Arc<DashMap<String, Instant>>,
}

impl TokenBlacklist {
    /// 创建黑名单（Redis 优先）
    pub fn new(redis: Option<RedisClient>) -> Self {
        Self {
            redis,
            local: Arc::new(DashMap::new()),
        }
    }

    /// 将 token 加入黑名单。Redis 写失败必须由调用者处理。
    pub async fn try_blacklist(&self, jti: &str, ttl_seconds: u64) -> AppResult<()> {
        if let Some(ref redis) = self.redis {
            let key = blacklist_key(jti);
            redis
                .set_ex(key, "1", ttl_seconds)
                .await
                .map_err(redis_unavailable)?;
        } else {
            // 内存模式：记录过期时刻
            let expiry = Instant::now() + Duration::from_secs(ttl_seconds);
            self.local.insert(jti.to_string(), expiry);
        }
        Ok(())
    }

    /// 检查 token 是否在黑名单中
    pub async fn is_blacklisted(&self, jti: &str) -> bool {
        self.try_is_blacklisted(jti).await.unwrap_or(true)
    }

    pub async fn try_is_blacklisted(&self, jti: &str) -> AppResult<bool> {
        if let Some(ref redis) = self.redis {
            let key = blacklist_key(jti);
            redis.exists(key).await.map_err(redis_unavailable)
        } else {
            // 内存模式：检查是否过期
            match self.local.get(jti) {
                Some(entry) => {
                    if entry.value() < &Instant::now() {
                        // 惰性删除过期条目
                        drop(entry);
                        self.local.remove(jti);
                        Ok(false)
                    } else {
                        Ok(true)
                    }
                }
                None => Ok(false),
            }
        }
    }

    /// 从黑名单中移除指定 key。Redis 写失败必须由调用者处理。
    pub async fn try_remove(&self, jti: &str) -> AppResult<()> {
        if let Some(ref redis) = self.redis {
            let key = blacklist_key(jti);
            redis.del(key).await.map_err(redis_unavailable)?;
        } else {
            self.local.remove(jti);
        }
        Ok(())
    }

    /// 启动后台 GC（仅内存模式需要）
    ///
    /// 每 60 秒清理一次过期条目。
    pub fn spawn_gc(&self) {
        let local = self.local.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let now = Instant::now();
                local.retain(|_, expiry| *expiry > now);
            }
        });
    }
}

fn redis_unavailable(error: redis::RedisError) -> AppError {
    tracing::error!(%error, "token blacklist Redis operation failed");
    AppError::ServiceUnavailable("session revocation service unavailable".into())
}

/// 生成 Redis key
pub fn blacklist_key(jti: &str) -> String {
    format!("ryframe:v0.5:access-revocation:{jti}")
}
