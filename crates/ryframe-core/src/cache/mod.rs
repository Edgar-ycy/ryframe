//! Unified cache abstraction and protection utilities.
//!
//! The module exposes a common [`Cache`] contract, Redis/local/no-op backends,
//! and opt-in protection layers for avalanche, penetration, breakdown, and
//! warm-up scenarios. Authorization is intentionally resolved from MySQL on
//! every request so a failed Redis invalidation cannot preserve old access.
//!
//! # Example
//!
//! ```
//! # use ryframe_core::cache::{Cache, CacheStrategy, LocalMemoryCache};
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
//! assert_eq!(value.as_deref(), Some("value"));
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

/// Common contract implemented by every cache backend and protection layer.
#[async_trait]
pub trait Cache: Send + Sync {
    /// Read and deserialize a cache value.
    async fn get<T: DeserializeOwned + Send>(&self, key: &str) -> Result<Option<T>, CacheError>;

    /// Serialize and store a value. A zero TTL means that it does not expire.
    async fn set<T: Serialize + Send + Sync>(
        &self,
        key: &str,
        value: &T,
        ttl_secs: u64,
    ) -> Result<(), CacheError>;

    /// Delete one cache entry.
    async fn delete(&self, key: &str) -> Result<(), CacheError>;

    /// Return whether a live cache entry exists.
    async fn exists(&self, key: &str) -> Result<bool, CacheError>;

    /// Return all cache keys matching a prefix.
    async fn keys(&self, prefix: &str) -> Result<Vec<String>, CacheError>;

    /// Delete all entries matching a prefix.
    async fn delete_by_prefix(&self, prefix: &str) -> Result<u64, CacheError> {
        let keys = self.keys(prefix).await?;
        let mut count = 0;
        for key in keys {
            self.delete(&key).await?;
            count += 1;
        }
        Ok(count)
    }

    /// Read a value or load and cache it on a miss.
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

/// Errors returned by cache serialization, backends, and protection layers.
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
