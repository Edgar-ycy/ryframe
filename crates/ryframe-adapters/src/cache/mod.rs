//! 统一缓存抽象与防护工具。
//!
//! 本模块提供通用 [`Cache`] 契约、Redis/本地/空操作后端，以及缓存雪崩、穿透、击穿和预热的
//! 可选防护层。授权信息在每个请求中均从 MySQL 读取，避免 Redis 失效失败保留过期访问权限。
//!
//! # 示例
//!
//! ```text
//! # use ryframe_adapters::cache::{Cache, CacheStrategy, LocalMemoryCache};
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let cache = CacheStrategy::new(LocalMemoryCache::unlimited())
//!     .with_avalanche_jitter(0.2)
//!     .with_null_cache_ttl(60);
//!
//! let value = cache
//!     .get_or_load_with_protection("example:key", 3600, || async {
//!         Ok(Some("value".to_string()))
//!     })
//!     .await?;
//! # Ok(())
//! # }
//! ```

mod backend;
mod breakdown;
mod entry;
mod strategy;
mod warmer;

use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};

pub use backend::{CacheBackend, LocalMemoryCache, NoopCache, RedisCache};
pub use breakdown::BreakdownGuard;
pub use strategy::{CacheStrategy, CacheStrategyConfig};
pub use warmer::{CacheWarmer, WarmUpTask};

/// 所有缓存后端和防护层共同实现的契约。
#[async_trait]
pub trait Cache: Send + Sync {
    /// 读取并反序列化缓存值。
    async fn get<T: DeserializeOwned + Send>(&self, key: &str) -> Result<Option<T>, CacheError>;

    /// 序列化并存储一个值。TTL 为零表示永不过期。
    async fn set<T: Serialize + Send + Sync>(
        &self,
        key: &str,
        value: &T,
        ttl_secs: u64,
    ) -> Result<(), CacheError>;

    /// 删除一个缓存条目。
    async fn delete(&self, key: &str) -> Result<(), CacheError>;

    /// 返回有效缓存条目是否存在。
    async fn exists(&self, key: &str) -> Result<bool, CacheError>;

    /// 返回匹配前缀的所有缓存键。
    async fn keys(&self, prefix: &str) -> Result<Vec<String>, CacheError>;

    /// 删除匹配前缀的所有条目。
    async fn delete_by_prefix(&self, prefix: &str) -> Result<u64, CacheError> {
        let keys = self.keys(prefix).await?;
        let mut count = 0;
        for key in keys {
            self.delete(&key).await?;
            count += 1;
        }
        Ok(count)
    }

    /// 读取一个值；未命中时加载并写入缓存。
    async fn get_or_load<T, F, Fut>(
        &self,
        key: &str,
        ttl_secs: u64,
        loader: F,
    ) -> Result<T, CacheError>
    where
        T: Serialize + DeserializeOwned + Send + Sync,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<T, CacheError>> + Send,
    {
        if let Some(cached) = self.get::<T>(key).await? {
            return Ok(cached);
        }

        let value = loader().await?;
        self.set(key, &value, ttl_secs).await?;
        Ok(value)
    }
}

/// 缓存序列化、后端和防护层返回的错误。
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("cache serialization failed: {0}")]
    Serialize(String),
    #[error("cache deserialization failed: {0}")]
    Deserialize(String),
    #[error("Redis operation failed: {0}")]
    Redis(String),
    #[error("cache operation failed: {0}")]
    Operation(String),
}

impl From<redis::RedisError> for CacheError {
    fn from(error: redis::RedisError) -> Self {
        Self::Redis(error.to_string())
    }
}
