use async_trait::async_trait;
use rand::RngExt;
use serde::{Serialize, de::DeserializeOwned};

use super::{
    Cache, CacheError,
    entry::{self, CacheLookup},
};

/// Configuration for cache avalanche and penetration protection.
#[derive(Clone, Debug)]
pub struct CacheStrategyConfig {
    /// Random TTL variation in the inclusive range `0.0..=0.5`.
    pub avalanche_jitter: f64,
    /// TTL for a cached null result. Zero disables null caching.
    pub null_cache_ttl: u64,
}

impl Default for CacheStrategyConfig {
    fn default() -> Self {
        Self {
            avalanche_jitter: 0.1,
            null_cache_ttl: 60,
        }
    }
}

/// Cache proxy adding randomized TTLs and typed null-value caching.
pub struct CacheStrategy<C: Cache> {
    inner: C,
    config: CacheStrategyConfig,
}

impl<C: Cache> CacheStrategy<C> {
    pub fn new(inner: C) -> Self {
        Self {
            inner,
            config: CacheStrategyConfig::default(),
        }
    }

    pub fn with_avalanche_jitter(mut self, jitter: f64) -> Self {
        self.config.avalanche_jitter = jitter.clamp(0.0, 0.5);
        self
    }

    pub fn with_null_cache_ttl(mut self, ttl_secs: u64) -> Self {
        self.config.null_cache_ttl = ttl_secs;
        self
    }

    pub fn inner(&self) -> &C {
        &self.inner
    }

    fn jittered_ttl(&self, base_ttl: u64) -> u64 {
        if self.config.avalanche_jitter == 0.0 || base_ttl == 0 {
            return base_ttl;
        }

        let jitter = self.config.avalanche_jitter;
        let factor = 1.0 - jitter + rand::rng().random_range(0.0..jitter * 2.0);
        (base_ttl as f64 * factor).round().max(1.0) as u64
    }

    /// Load a typed value on a cache miss while protecting null results.
    pub async fn get_or_load_with_protection<T, F, Fut>(
        &self,
        key: &str,
        ttl_secs: u64,
        loader: F,
    ) -> Result<Option<T>, CacheError>
    where
        T: Serialize + DeserializeOwned + Send + Sync,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<Option<T>, CacheError>> + Send,
    {
        match entry::read(&self.inner, key).await? {
            CacheLookup::Value(value) => return Ok(Some(value)),
            CacheLookup::Null => return Ok(None),
            CacheLookup::Miss => {}
        }

        match loader().await? {
            Some(value) => {
                entry::write_value(&self.inner, key, &value, self.jittered_ttl(ttl_secs)).await?;
                Ok(Some(value))
            }
            None => {
                if self.config.null_cache_ttl > 0 {
                    entry::write_null(&self.inner, key, self.config.null_cache_ttl).await?;
                }
                Ok(None)
            }
        }
    }
}

#[async_trait]
impl<C: Cache> Cache for CacheStrategy<C> {
    async fn get<T: DeserializeOwned + Send>(&self, key: &str) -> Result<Option<T>, CacheError> {
        Ok(entry::read(&self.inner, key).await?.into_option())
    }

    async fn set<T: Serialize + Send + Sync>(
        &self,
        key: &str,
        value: &T,
        ttl_secs: u64,
    ) -> Result<(), CacheError> {
        entry::write_value(&self.inner, key, value, self.jittered_ttl(ttl_secs)).await
    }

    async fn delete(&self, key: &str) -> Result<(), CacheError> {
        self.inner.delete(key).await
    }

    async fn exists(&self, key: &str) -> Result<bool, CacheError> {
        self.inner.exists(key).await
    }

    async fn keys(&self, prefix: &str) -> Result<Vec<String>, CacheError> {
        self.inner.keys(prefix).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use serde::Deserialize;

    use super::*;
    use crate::cache::LocalMemoryCache;

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct ExampleValue {
        id: i64,
        name: String,
    }

    #[tokio::test]
    async fn strategy_round_trips_non_string_values() {
        let cache = CacheStrategy::new(LocalMemoryCache::unlimited()).with_avalanche_jitter(0.0);
        let expected = ExampleValue {
            id: 7,
            name: "cached".to_owned(),
        };

        cache.set("value", &expected, 60).await.unwrap();

        assert_eq!(
            cache.get::<ExampleValue>("value").await.unwrap(),
            Some(expected)
        );
    }

    #[tokio::test]
    async fn strategy_caches_null_results() {
        let cache = CacheStrategy::new(LocalMemoryCache::unlimited())
            .with_null_cache_ttl(60)
            .with_avalanche_jitter(0.0);
        let loads = Arc::new(AtomicUsize::new(0));

        for _ in 0..2 {
            let loads = Arc::clone(&loads);
            let result = cache
                .get_or_load_with_protection::<String, _, _>("missing", 60, move || async move {
                    loads.fetch_add(1, Ordering::SeqCst);
                    Ok(None)
                })
                .await
                .unwrap();
            assert_eq!(result, None);
        }

        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn jitter_stays_inside_configured_range() {
        let cache = CacheStrategy::new(crate::cache::NoopCache);
        let ttl = cache.jittered_ttl(100);

        assert!((90..=110).contains(&ttl), "unexpected jittered TTL: {ttl}");
    }
}
