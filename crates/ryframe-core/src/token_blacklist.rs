//! Token 黑名单
//!
//! 用于实现 JWT 主动撤销：
//! - Redis 模式：存储 jti → "1"，设置 TTL = token 剩余有效时间
//! - 内存模式（Redis 不可用时降级）：DashMap 存储，后台 GC 清理过期

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use dashmap::DashMap;

use crate::RedisClient;

/// Token 黑名单
///
/// 用于在登出时主动撤销 JWT，防止令牌在有效期内被滥用。
/// Redis 模式为首选（支持分布式），Redis 不可用时自动降级为内存模式。
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

    /// 将 token 加入黑名单
    ///
    /// `jti` - JWT ID，token 的唯一标识
    /// `ttl_seconds` - 黑名单有效时长（应设置为 token 的剩余有效时间）
    pub async fn blacklist(&self, jti: &str, ttl_seconds: u64) {
        if let Some(ref redis) = self.redis {
            let key = blacklist_key(jti);
            let _ = redis.set_ex(key, "1", ttl_seconds).await;
        } else {
            // 内存模式：记录过期时刻
            let expiry = Instant::now() + Duration::from_secs(ttl_seconds);
            self.local.insert(jti.to_string(), expiry);
        }
    }

    /// 检查 token 是否在黑名单中
    pub async fn is_blacklisted(&self, jti: &str) -> bool {
        if let Some(ref redis) = self.redis {
            let key = blacklist_key(jti);
            redis.exists(key).await.unwrap_or(false)
        } else {
            // 内存模式：检查是否过期
            match self.local.get(jti) {
                Some(entry) => {
                    if entry.value() < &Instant::now() {
                        // 惰性删除过期条目
                        drop(entry);
                        self.local.remove(jti);
                        false
                    } else {
                        true
                    }
                }
                None => false,
            }
        }
    }

    /// 从黑名单中移除指定 key（用于登录后清除强退标记等）
    pub async fn remove(&self, jti: &str) {
        if let Some(ref redis) = self.redis {
            let key = blacklist_key(jti);
            let _ = redis.del(key).await;
        } else {
            self.local.remove(jti);
        }
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

/// 生成 Redis key
pub fn blacklist_key(jti: &str) -> String {
    format!("token:blacklist:{}", jti)
}
