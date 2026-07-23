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

struct LockRegistration {
    locks: Arc<DashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    key: String,
    mutex: Option<Arc<tokio::sync::Mutex<()>>>,
}

impl LockRegistration {
    fn mutex(&self) -> &tokio::sync::Mutex<()> {
        self.mutex.as_deref().expect("lock registration is active")
    }
}

impl Drop for LockRegistration {
    fn drop(&mut self) {
        // Release this caller's ownership before checking whether the map is
        // the sole remaining owner. This also runs when the future is aborted
        // or unwinds, so cancellation cannot strand registrations.
        drop(self.mutex.take());
        self.locks.remove_if(&self.key, |_, registered| {
            Arc::strong_count(registered) == 1
        });
    }
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

        let registration = LockRegistration {
            locks: Arc::clone(&self.locks),
            key: key.to_owned(),
            mutex: Some(self.get_mutex(key)),
        };
        match tokio::time::timeout(self.wait_timeout, registration.mutex().lock()).await {
            Ok(lock_guard) => {
                let result = async {
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
                .await;
                drop(lock_guard);
                result
            }
            Err(_) => {
                tracing::warn!(cache_key = key, "cache breakdown lock timed out");
                Ok(entry::read(&self.inner, key).await?.into_option())
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

    #[tokio::test]
    async fn lock_registrations_are_released_after_unique_requests() {
        let guard = BreakdownGuard::new(LocalMemoryCache::unlimited());

        for index in 0..1_000 {
            let key = format!("key:{index}");
            assert_eq!(
                guard
                    .get_or_load_guarded(&key, 60, || async { Ok(Some(index)) })
                    .await
                    .unwrap(),
                Some(index)
            );
        }

        assert!(guard.locks.is_empty());
    }

    #[tokio::test]
    async fn lock_registration_is_released_after_loader_error() {
        let guard = BreakdownGuard::new(LocalMemoryCache::unlimited());

        let result = guard
            .get_or_load_guarded::<String, _, _>("failing", 60, || async {
                Err(CacheError::Operation("loader failed".into()))
            })
            .await;

        assert!(matches!(result, Err(CacheError::Operation(_))));
        assert!(guard.locks.is_empty());
    }

    #[tokio::test]
    async fn last_waiter_cleans_registration_after_timing_out() {
        let guard = Arc::new(
            BreakdownGuard::new(LocalMemoryCache::unlimited())
                .with_wait_timeout(Duration::from_millis(5)),
        );
        let loader_started = Arc::new(tokio::sync::Notify::new());
        let release_loader = Arc::new(tokio::sync::Notify::new());

        let owner = {
            let guard = Arc::clone(&guard);
            let loader_started = Arc::clone(&loader_started);
            let release_loader = Arc::clone(&release_loader);
            tokio::spawn(async move {
                guard
                    .get_or_load_guarded("slow", 60, || async move {
                        loader_started.notify_one();
                        release_loader.notified().await;
                        Ok(Some("loaded".to_owned()))
                    })
                    .await
            })
        };

        loader_started.notified().await;
        assert_eq!(
            guard
                .get_or_load_guarded::<String, _, _>("slow", 60, || async {
                    panic!("timed-out waiter must not run the loader")
                })
                .await
                .unwrap(),
            None
        );

        release_loader.notify_one();
        assert_eq!(owner.await.unwrap().unwrap().as_deref(), Some("loaded"));
        assert!(guard.locks.is_empty());
    }

    #[tokio::test]
    async fn aborted_loader_does_not_strand_registration() {
        let guard = Arc::new(BreakdownGuard::new(LocalMemoryCache::unlimited()));
        let loader_started = Arc::new(tokio::sync::Notify::new());

        let owner = {
            let guard = Arc::clone(&guard);
            let loader_started = Arc::clone(&loader_started);
            tokio::spawn(async move {
                guard
                    .get_or_load_guarded::<String, _, _>("cancelled", 60, || async move {
                        loader_started.notify_one();
                        std::future::pending().await
                    })
                    .await
            })
        };

        loader_started.notified().await;
        owner.abort();
        assert!(owner.await.unwrap_err().is_cancelled());
        assert!(guard.locks.is_empty());
    }
}
