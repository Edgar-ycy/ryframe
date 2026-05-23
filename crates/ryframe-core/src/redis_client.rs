//! Redis 客户端封装
//!
//! 提供异步 Redis 连接管理器和常用操作封装。
//! 当 Redis 未配置时，调用方应回退到内存存储。

use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use ryframe_config::RedisConfig;
use std::time::Duration;

/// Redis 客户端封装
///
/// 内部使用 `ConnectionManager`，自动处理重连和连接池管理。
#[derive(Clone)]
pub struct RedisClient {
    conn: ConnectionManager,
}

impl RedisClient {
    /// 从配置创建 Redis 客户端
    ///
    /// # Errors
    /// 连接超时或 Redis 不可达时返回错误
    pub async fn connect(config: &RedisConfig) -> Result<Self, redis::RedisError> {
        let client = redis::Client::open(config.connection_url())?;

        // 带超时的连接
        let conn = tokio::time::timeout(
            Duration::from_secs(config.timeout_secs),
            ConnectionManager::new(client),
        )
        .await
        .map_err(|_| {
            redis::RedisError::from(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "Redis 连接超时 ({}s): {}:{}",
                    config.timeout_secs, config.host, config.port
                ),
            ))
        })??;

        tracing::info!("Redis 连接成功: {}:{}", config.host, config.port);
        Ok(Self { conn })
    }

    /// 获取底层连接管理器（用于高级操作）
    pub fn conn(&self) -> &ConnectionManager {
        &self.conn
    }

    // ========== 便捷方法 ==========

    /// SET key value（无过期）
    pub async fn set<K: AsRef<str>, V: AsRef<str>>(
        &self,
        key: K,
        value: V,
    ) -> Result<(), redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.set(key.as_ref(), value.as_ref()).await
    }

    /// SET key value EX seconds（带过期时间）
    pub async fn set_ex<K: AsRef<str>, V: AsRef<str>>(
        &self,
        key: K,
        value: V,
        seconds: u64,
    ) -> Result<(), redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.set_ex(key.as_ref(), value.as_ref(), seconds).await
    }

    /// GET key（不存在返回 None）
    pub async fn get<K: AsRef<str>>(&self, key: K) -> Result<Option<String>, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.get(key.as_ref()).await
    }

    /// DEL key（删除键，返回删除数量）
    pub async fn del<K: AsRef<str>>(&self, key: K) -> Result<u64, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.del(key.as_ref()).await
    }

    /// GET + DEL（原子获取并删除，模拟一次性读取）
    ///
    /// 注意：非原子操作，但在验证码场景可接受
    pub async fn get_and_del<K: AsRef<str>>(
        &self,
        key: K,
    ) -> Result<Option<String>, redis::RedisError> {
        let value = self.get(key.as_ref()).await?;
        if value.is_some() {
            self.del(key.as_ref()).await?;
        }
        Ok(value)
    }

    /// EXISTS key
    pub async fn exists<K: AsRef<str>>(&self, key: K) -> Result<bool, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.exists(key.as_ref()).await
    }

    /// TTL key（返回剩余秒数，-1=永不过期，-2=不存在）
    pub async fn ttl<K: AsRef<str>>(&self, key: K) -> Result<i64, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.ttl(key.as_ref()).await
    }

    /// PING
    pub async fn ping(&self) -> Result<String, redis::RedisError> {
        let mut conn = self.conn.clone();
        redis::cmd("PING").query_async(&mut conn).await
    }

    /// KEYS pattern（生产环境建议用 SCAN）
    pub async fn keys<K: AsRef<str>>(&self, pattern: K) -> Result<Vec<String>, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.keys(pattern.as_ref()).await
    }

    /// HSET key field value
    pub async fn hset<K: AsRef<str>, F: AsRef<str>, V: AsRef<str>>(
        &self,
        key: K,
        field: F,
        value: V,
    ) -> Result<(), redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.hset(key.as_ref(), field.as_ref(), value.as_ref())
            .await
    }

    /// HGETALL key（返回 HashMap）
    pub async fn hgetall<K: AsRef<str>>(
        &self,
        key: K,
    ) -> Result<std::collections::HashMap<String, String>, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.hgetall(key.as_ref()).await
    }

    /// HDEL key field
    pub async fn hdel<K: AsRef<str>, F: AsRef<str>>(
        &self,
        key: K,
        field: F,
    ) -> Result<u64, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.hdel(key.as_ref(), field.as_ref()).await
    }

    /// EXPIRE key seconds
    pub async fn expire<K: AsRef<str>>(
        &self,
        key: K,
        seconds: u64,
    ) -> Result<bool, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.expire(key.as_ref(), seconds as i64).await
    }

    /// INCR key（原子递增）
    pub async fn incr<K: AsRef<str>>(&self, key: K) -> Result<i64, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.incr(key.as_ref(), 1).await
    }

    /// DECR key（原子递减）
    pub async fn decr<K: AsRef<str>>(&self, key: K) -> Result<i64, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.decr(key.as_ref(), 1).await
    }
}

/// 根据配置创建 Redis 客户端
///
/// - 配置了 Redis → 尝试连接，成功返回 Some，失败返回 None（降级到内存模式）
/// - 未配置 Redis → 返回 None
pub async fn create_redis_client(config: &Option<RedisConfig>) -> Option<RedisClient> {
    let redis_config = config.as_ref()?;
    match RedisClient::connect(redis_config).await {
        Ok(client) => {
            // 验证连接
            match client.ping().await {
                Ok(_) => {
                    tracing::info!("Redis 服务就绪，启用 Redis 模式");
                    Some(client)
                }
                Err(e) => {
                    tracing::warn!("Redis PING 失败，降级到内存模式: {}", e);
                    None
                }
            }
        }
        Err(e) => {
            tracing::warn!("Redis 连接失败，降级到内存模式: {}", e);
            None
        }
    }
}
