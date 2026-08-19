use std::{sync::Arc, time::Duration};

use dashmap::DashMap;
use serde::{Serialize, de::DeserializeOwned};

use super::{
    Cache, CacheError,
    entry::{self, CacheLookup},
};

/// 面向热点缓存条目的按键双重检查锁。
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
        // 在检查映射是否为唯一剩余持有者之前，先释放当前调用方的所有权。此逻辑在
        // 异步任务被中止或展开时同样执行，因此取消操作不会遗留注册项。
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

    /// 访问不相关键对应的后端。受守卫管理的键必须通过 [`Self::get_or_load_guarded`]
    /// 读写。
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

    /// 每次仅加载一个热点键，并让并发调用方消费其结果。
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

    /// 移除不再被任何请求使用的互斥锁注册项。
    pub fn clean_stale_locks(&self) {
        self.locks.retain(|_, mutex| Arc::strong_count(mutex) > 1);
    }
}
