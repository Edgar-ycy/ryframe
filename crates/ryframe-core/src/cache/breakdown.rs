use std::{sync::Arc, time::Duration};

use dashmap::DashMap;
use serde::{Serialize, de::DeserializeOwned};

use super::{
    Cache, CacheError,
    entry::{self, CacheLookup},
};

/// Per-key double-checked locking for hot cache entries.
pub struct BreakdownGuard<C: Cache> {
    inner: C,
    locks: Arc<DashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    wait_timeout: Duration,
    null_cache_ttl: u64,
}

impl<C: Cache> BreakdownGuard<C> {
    pub fn new(inner: C) -> Self {
        Self {
            inner,
            locks: Arc::new(DashMap::new()),
            wait_timeout: Duration::from_secs(10),
            null_cache_ttl: 60,
        }
    }

    pub fn with_wait_timeout(mut self, timeout: Duration) -> Self {
        self.wait_timeout = timeout;
        self
    }

    pub fn with_null_cache_ttl(mut self, ttl_secs: u64) -> Self {
        self.null_cache_ttl = ttl_secs;
        self
    }

    /// Access the backend for unrelated keys. Guard-managed keys must be read
    /// and written through [`Self::get_or_load_guarded`].
    pub fn inner(&self) -> &C {
        &self.inner
    }

    pub fn get_mutex(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.locks
            .entry(key.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .value()
            .clone()
    }

    /// Load one hot key at a time and let concurrent callers consume its result.
    pub async fn get_or_load_guarded<T, F, Fut>(
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

        let mutex = self.get_mutex(key);
        let _guard = match tokio::time::timeout(self.wait_timeout, mutex.lock()).await {
            Ok(guard) => guard,
            Err(_) => {
                tracing::warn!(cache_key = key, "cache breakdown lock timed out");
                return Ok(entry::read(&self.inner, key).await?.into_option());
            }
        };

        match entry::read(&self.inner, key).await? {
            CacheLookup::Value(value) => return Ok(Some(value)),
            CacheLookup::Null => return Ok(None),
            CacheLookup::Miss => {}
        }

        match loader().await? {
            Some(value) => {
                entry::write_value(&self.inner, key, &value, ttl_secs).await?;
                Ok(Some(value))
            }
            None => {
                if self.null_cache_ttl > 0 {
                    entry::write_null(&self.inner, key, self.null_cache_ttl).await?;
                }
                Ok(None)
            }
        }
    }

    /// Remove mutex registrations that are no longer used by a request.
    pub fn clean_stale_locks(&self) {
        self.locks.retain(|_, mutex| Arc::strong_count(mutex) > 1);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::cache::LocalMemoryCache;

    #[tokio::test]
    async fn concurrent_misses_share_one_loader() {
        let guard = BreakdownGuard::new(LocalMemoryCache::unlimited());
        let loads = AtomicUsize::new(0);

        let load = || async {
            loads.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok(Some("value".to_owned()))
        };
        let (first, second) = tokio::join!(
            guard.get_or_load_guarded("hot", 60, load),
            guard.get_or_load_guarded("hot", 60, load),
        );

        assert_eq!(first.unwrap().as_deref(), Some("value"));
        assert_eq!(second.unwrap().as_deref(), Some("value"));
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn null_results_are_not_reloaded() {
        let guard = BreakdownGuard::new(LocalMemoryCache::unlimited());
        let loads = AtomicUsize::new(0);

        for _ in 0..2 {
            let result = guard
                .get_or_load_guarded::<String, _, _>("missing", 60, || async {
                    loads.fetch_add(1, Ordering::SeqCst);
                    Ok(None)
                })
                .await
                .unwrap();
            assert_eq!(result, None);
        }

        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }
}
