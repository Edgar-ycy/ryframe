use std::{future::Future, pin::Pin, sync::Arc};

use super::{Cache, CacheError};

type WarmUpFuture = Pin<Box<dyn Future<Output = Result<String, CacheError>> + Send>>;
type WarmUpLoader = Arc<dyn Fn() -> WarmUpFuture + Send + Sync>;

/// One cache warm-up definition.
#[derive(Clone)]
pub struct WarmUpTask<C: Cache> {
    pub key: String,
    pub ttl_secs: u64,
    loader: WarmUpLoader,
    cache_type: std::marker::PhantomData<C>,
}

impl<C: Cache> std::fmt::Debug for WarmUpTask<C> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WarmUpTask")
            .field("key", &self.key)
            .field("ttl_secs", &self.ttl_secs)
            .finish_non_exhaustive()
    }
}

/// Concurrently loads configured hot values into a cache.
pub struct CacheWarmer<C: Cache> {
    cache: C,
    tasks: Vec<WarmUpTask<C>>,
}

impl<C: Cache + 'static> CacheWarmer<C> {
    pub fn new(cache: C) -> Self {
        Self {
            cache,
            tasks: Vec::new(),
        }
    }

    /// Register a warm-up task without requiring callers to box its future.
    pub fn add_task<F, Fut>(&mut self, key: impl Into<String>, ttl_secs: u64, loader: F)
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<String, CacheError>> + Send + 'static,
    {
        let loader = Arc::new(loader);
        self.tasks.push(WarmUpTask {
            key: key.into(),
            ttl_secs,
            loader: Arc::new(move || {
                let loader = Arc::clone(&loader);
                Box::pin(async move { loader().await })
            }),
            cache_type: std::marker::PhantomData,
        });
    }

    /// Execute all tasks and return `(successful, failed)` counts.
    pub async fn warm_up(&self) -> (usize, usize) {
        let handles: Vec<_> = self
            .tasks
            .iter()
            .map(|task| {
                let loader = Arc::clone(&task.loader);
                let key = task.key.clone();
                let ttl_secs = task.ttl_secs;
                tokio::spawn(async move { loader().await.map(|value| (key, value, ttl_secs)) })
            })
            .collect();

        let mut successful = 0;
        let mut failed = 0;
        for handle in handles {
            match handle.await {
                Ok(Ok((key, value, ttl_secs))) => {
                    if let Err(error) = self.cache.set(&key, &value, ttl_secs).await {
                        failed += 1;
                        tracing::warn!(cache_key = key, %error, "cache warm-up write failed");
                    } else {
                        successful += 1;
                        tracing::info!(cache_key = key, "cache warm-up completed");
                    }
                }
                Ok(Err(error)) => {
                    failed += 1;
                    tracing::warn!(%error, "cache warm-up loader failed");
                }
                Err(error) => {
                    failed += 1;
                    tracing::warn!(%error, "cache warm-up task panicked");
                }
            }
        }

        tracing::info!(
            successful,
            failed,
            total = self.tasks.len(),
            "cache warm-up finished"
        );
        (successful, failed)
    }

    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{Cache, LocalMemoryCache};

    #[tokio::test]
    async fn warmer_reports_each_task_and_stores_successes() {
        let cache = LocalMemoryCache::unlimited();
        let inspection_cache = cache.clone();
        let mut warmer = CacheWarmer::new(cache);
        warmer.add_task("ready", 60, || async { Ok("value".to_owned()) });
        warmer.add_task("failed", 60, || async {
            Err(CacheError::Operation("load failed".to_owned()))
        });

        assert_eq!(warmer.task_count(), 2);
        assert_eq!(warmer.warm_up().await, (1, 1));
        assert_eq!(
            inspection_cache
                .get::<String>("ready")
                .await
                .unwrap()
                .as_deref(),
            Some("value")
        );
    }
}
