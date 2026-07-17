use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::RwLock;

use crate::RedisClient;

use super::{Cache, CacheError};

/// Cache backend used when caching is disabled.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopCache;

#[async_trait]
impl Cache for NoopCache {
    async fn get<T: DeserializeOwned + Send>(&self, _key: &str) -> Result<Option<T>, CacheError> {
        Ok(None)
    }

    async fn set<T: Serialize + Send + Sync>(
        &self,
        _key: &str,
        _value: &T,
        _ttl_secs: u64,
    ) -> Result<(), CacheError> {
        Ok(())
    }

    async fn delete(&self, _key: &str) -> Result<(), CacheError> {
        Ok(())
    }

    async fn exists(&self, _key: &str) -> Result<bool, CacheError> {
        Ok(false)
    }

    async fn keys(&self, _prefix: &str) -> Result<Vec<String>, CacheError> {
        Ok(Vec::new())
    }
}

/// Redis-backed cache using JSON serialization.
#[derive(Clone)]
pub struct RedisCache {
    client: RedisClient,
}

impl RedisCache {
    pub fn new(client: RedisClient) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &RedisClient {
        &self.client
    }
}

#[async_trait]
impl Cache for RedisCache {
    async fn get<T: DeserializeOwned + Send>(&self, key: &str) -> Result<Option<T>, CacheError> {
        match self.client.get(key).await {
            Ok(Some(json)) => serde_json::from_str(&json).map(Some).map_err(|error| {
                CacheError::Deserialize(format!("failed to deserialize key {key}: {error}"))
            }),
            Ok(None) => Ok(None),
            Err(error) => Err(CacheError::Redis(format!(
                "GET failed for key {key}: {error}"
            ))),
        }
    }

    async fn set<T: Serialize + Send + Sync>(
        &self,
        key: &str,
        value: &T,
        ttl_secs: u64,
    ) -> Result<(), CacheError> {
        let json = serde_json::to_string(value)
            .map_err(|error| CacheError::Serialize(error.to_string()))?;

        if ttl_secs == 0 {
            self.client
                .set(key, &json)
                .await
                .map_err(|error| CacheError::Redis(format!("SET failed for key {key}: {error}")))?;
        } else {
            self.client
                .set_ex(key, &json, ttl_secs)
                .await
                .map_err(|error| {
                    CacheError::Redis(format!("SETEX failed for key {key}: {error}"))
                })?;
        }
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), CacheError> {
        self.client
            .del(key)
            .await
            .map_err(|error| CacheError::Redis(format!("DEL failed for key {key}: {error}")))?;
        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool, CacheError> {
        self.client
            .exists(key)
            .await
            .map_err(|error| CacheError::Redis(format!("EXISTS failed for key {key}: {error}")))
    }

    async fn keys(&self, prefix: &str) -> Result<Vec<String>, CacheError> {
        let pattern = format!("{prefix}*");
        self.client
            .scan_keys(&pattern)
            .await
            .map_err(|error| CacheError::Redis(format!("SCAN failed for {pattern}: {error}")))
    }
}

struct CachedEntry {
    value: String,
    expires_at: Option<Instant>,
    last_accessed_at: Instant,
}

/// Process-local cache for single-node deployments and graceful degradation.
///
/// A finite capacity uses least-recently-used eviction. Expired entries are
/// removed lazily by every operation that inspects the store.
#[derive(Clone)]
pub struct LocalMemoryCache {
    store: Arc<RwLock<HashMap<String, CachedEntry>>>,
    capacity: Option<usize>,
}

impl LocalMemoryCache {
    /// Create a local cache. Zero and `usize::MAX` mean unlimited capacity.
    pub fn new(capacity: usize) -> Self {
        let capacity = match capacity {
            0 | usize::MAX => None,
            value => Some(value),
        };
        Self {
            store: Arc::new(RwLock::new(HashMap::with_capacity(capacity.unwrap_or(0)))),
            capacity,
        }
    }

    pub fn unlimited() -> Self {
        Self::new(0)
    }

    /// Remove all expired entries immediately.
    pub async fn clean_expired(&self) {
        let mut store = self.store.write().await;
        Self::remove_expired(&mut store, Instant::now());
    }

    fn remove_expired(store: &mut HashMap<String, CachedEntry>, now: Instant) {
        store.retain(|_, entry| entry.expires_at.is_none_or(|expires_at| expires_at > now));
    }

    fn evict_if_full(&self, store: &mut HashMap<String, CachedEntry>, key: &str) {
        let Some(capacity) = self.capacity else {
            return;
        };
        if store.contains_key(key) || store.len() < capacity {
            return;
        }

        if let Some(eviction_key) = store
            .iter()
            .min_by_key(|(_, entry)| entry.last_accessed_at)
            .map(|(key, _)| key.clone())
        {
            store.remove(&eviction_key);
        }
    }
}

#[async_trait]
impl Cache for LocalMemoryCache {
    async fn get<T: DeserializeOwned + Send>(&self, key: &str) -> Result<Option<T>, CacheError> {
        let now = Instant::now();
        let mut store = self.store.write().await;
        if store
            .get(key)
            .is_some_and(|entry| entry.expires_at.is_some_and(|expires_at| expires_at <= now))
        {
            store.remove(key);
            return Ok(None);
        }

        let Some(entry) = store.get_mut(key) else {
            return Ok(None);
        };
        entry.last_accessed_at = now;
        serde_json::from_str(&entry.value)
            .map(Some)
            .map_err(|error| CacheError::Deserialize(error.to_string()))
    }

    async fn set<T: Serialize + Send + Sync>(
        &self,
        key: &str,
        value: &T,
        ttl_secs: u64,
    ) -> Result<(), CacheError> {
        let value = serde_json::to_string(value)
            .map_err(|error| CacheError::Serialize(error.to_string()))?;
        let now = Instant::now();
        let expires_at = (ttl_secs > 0).then(|| now + Duration::from_secs(ttl_secs));
        let mut store = self.store.write().await;
        Self::remove_expired(&mut store, now);
        self.evict_if_full(&mut store, key);
        store.insert(
            key.to_owned(),
            CachedEntry {
                value,
                expires_at,
                last_accessed_at: now,
            },
        );
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), CacheError> {
        self.store.write().await.remove(key);
        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool, CacheError> {
        let now = Instant::now();
        let mut store = self.store.write().await;
        if store
            .get(key)
            .is_some_and(|entry| entry.expires_at.is_some_and(|expires_at| expires_at <= now))
        {
            store.remove(key);
            return Ok(false);
        }
        Ok(store.contains_key(key))
    }

    async fn keys(&self, prefix: &str) -> Result<Vec<String>, CacheError> {
        let mut store = self.store.write().await;
        Self::remove_expired(&mut store, Instant::now());
        Ok(store
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect())
    }
}

/// Runtime-selected cache backend.
pub enum CacheBackend {
    Redis(Box<RedisCache>),
    Local(LocalMemoryCache),
    Noop(NoopCache),
}

impl CacheBackend {
    /// Prefer Redis when configured, otherwise use process-local storage.
    pub fn from_redis(redis: Option<RedisClient>) -> Self {
        match redis {
            Some(client) => Self::Redis(Box::new(RedisCache::new(client))),
            None => Self::Local(LocalMemoryCache::unlimited()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_cache_supports_basic_operations() {
        let cache = LocalMemoryCache::unlimited();
        cache.set("key", &"value", 60).await.unwrap();

        assert_eq!(
            cache.get::<String>("key").await.unwrap().as_deref(),
            Some("value")
        );
        assert!(cache.exists("key").await.unwrap());

        cache.delete("key").await.unwrap();
        assert!(!cache.exists("key").await.unwrap());
        assert_eq!(cache.get::<String>("missing").await.unwrap(), None);
    }

    #[tokio::test]
    async fn local_cache_removes_expired_entries_from_all_views() {
        let cache = LocalMemoryCache::unlimited();
        cache.set("expired", &"value", 1).await.unwrap();
        tokio::time::sleep(Duration::from_millis(1100)).await;

        assert!(!cache.exists("expired").await.unwrap());
        assert!(cache.keys("").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn local_cache_enforces_capacity_with_lru_eviction() {
        let cache = LocalMemoryCache::new(2);
        cache.set("old", &1, 0).await.unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
        cache.set("recent", &2, 0).await.unwrap();
        let _: Option<i32> = cache.get("old").await.unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
        cache.set("new", &3, 0).await.unwrap();

        assert!(cache.exists("old").await.unwrap());
        assert!(!cache.exists("recent").await.unwrap());
        assert!(cache.exists("new").await.unwrap());
    }

    #[tokio::test]
    async fn noop_cache_always_misses() {
        let cache = NoopCache;
        cache.set("key", &"value", 60).await.unwrap();

        assert_eq!(cache.get::<String>("key").await.unwrap(), None);
        assert!(!cache.exists("key").await.unwrap());
    }
}
