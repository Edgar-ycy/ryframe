//! 分布式锁
//!
//! 基于 Redis SET NX 实现分布式锁，用于多实例场景下的资源互斥访问。
//! 支持 RAII 模式自动释放，防止死锁。
//!
//! # 使用示例
//!
//! ```text
//! use ryframe_adapters::distributed_lock::{DistributedLock, LocalDistributedLock};
//! use std::time::Duration;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let lock = LocalDistributedLock::new();
//! if let Some(guard) = lock.try_acquire("my_resource", Duration::from_secs(30)).await? {
//!     // 获取到锁，执行临界区代码
//!     let _ = do_critical_work();
//!     // guard 在 drop 时自动释放锁
//! }
//! # Ok(())
//! # }
//! #
//! # fn do_critical_work() -> i32 { 42 }
//! ```

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use dashmap::{DashMap, mapref::entry::Entry};
use redis::{AsyncCommands, SetExpiry, SetOptions};
use ryframe_kernel::{AppError, AppResult};

use crate::redis_client::RedisClient;

const LOCK_KEY_PREFIX: &str = "ryframe:v0.5:lock:";

/// 分布式锁 trait
///
/// 所有分布式锁实现（Redis、ZooKeeper、etcd 等）都应实现此 trait。
#[async_trait::async_trait]
pub trait DistributedLock: Send + Sync {
    /// 尝试获取锁
    ///
    /// - `key`: 锁的键名
    /// - `ttl`: 锁的过期时间（防止持有者崩溃导致死锁）
    /// - 返回 `Some(LockGuard)` 表示获取成功，`None` 表示锁已被其他实例持有
    async fn try_acquire(&self, key: &str, ttl: Duration) -> AppResult<Option<LockGuard>>;

    /// 尝试获取锁（带重试）
    ///
    /// 在指定时间内每隔 `retry_interval` 重试一次。
    async fn try_acquire_with_retry(
        &self,
        key: &str,
        ttl: Duration,
        max_wait: Duration,
        retry_interval: Duration,
    ) -> AppResult<Option<LockGuard>> {
        let deadline = tokio::time::Instant::now() + max_wait;
        loop {
            match self.try_acquire(key, ttl).await? {
                Some(guard) => return Ok(Some(guard)),
                None => {
                    if tokio::time::Instant::now() >= deadline {
                        return Ok(None);
                    }
                    tokio::time::sleep(retry_interval).await;
                }
            }
        }
    }

    /// 强制释放锁（忽略持有者身份）
    ///
    /// 用于管理员手动释放孤立锁。
    async fn force_release(&self, key: &str) -> AppResult<bool>;
}

/// 锁守卫（RAII）
///
/// 获取锁后返回此对象，在其被 drop 时自动释放锁。
///
/// # 使用方式
///
/// ```text
/// # use ryframe_adapters::distributed_lock::{DistributedLock, LocalDistributedLock};
/// # use std::time::Duration;
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let lock = LocalDistributedLock::new();
/// let ttl = Duration::from_secs(30);
/// {
///     let guard = lock.try_acquire("task:clean_log", ttl).await?.unwrap();
///     // guard 离开作用域时自动释放锁
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct LockGuard {
    inner: Arc<LockGuardInner>,
}

struct LockGuardInner {
    release_fn: Box<dyn Fn() -> AppResult<()> + Send + Sync>,
    key: String,
    holder_id: String,
    released: AtomicBool,
}

impl std::fmt::Debug for LockGuardInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LockGuardInner")
            .field("release_fn", &"<closure>")
            .field("key", &self.key)
            .field("holder_id", &self.holder_id)
            .finish()
    }
}

impl LockGuard {
    /// 锁的键名
    pub fn key(&self) -> &str {
        &self.inner.key
    }

    /// 持有者标识
    pub fn holder_id(&self) -> &str {
        &self.inner.holder_id
    }

    /// 手动释放锁（通常不需要，drop 时自动调用）
    pub fn release(self) -> AppResult<()> {
        self.inner.release_once()
    }
}

impl LockGuardInner {
    fn release_once(&self) -> AppResult<()> {
        if self.released.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        (self.release_fn)()
    }
}

impl Drop for LockGuardInner {
    fn drop(&mut self) {
        if let Err(e) = self.release_once() {
            tracing::warn!("分布式锁释放失败 [key={}]: {}", self.key, e);
        }
    }
}

/// 基于 Redis 的分布式锁实现
///
/// 使用 `SET key value NX EX ttl` 命令实现原子性获取锁。
#[derive(Clone)]
pub struct RedisDistributedLock {
    client: RedisClient,
}

impl RedisDistributedLock {
    /// 创建 Redis 分布式锁
    pub fn new(client: RedisClient) -> Self {
        Self { client }
    }

    /// 获取底层 Redis 客户端引用
    pub fn client(&self) -> &RedisClient {
        &self.client
    }

    /// 生成唯一持有者 ID（时间戳 + 进程ID）
    pub fn holder_id() -> String {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros();
        let pid = std::process::id();
        format!("{}-{}-{:032x}", timestamp, pid, rand::random::<u128>())
    }
}

#[async_trait::async_trait]
impl DistributedLock for RedisDistributedLock {
    async fn try_acquire(&self, key: &str, ttl: Duration) -> AppResult<Option<LockGuard>> {
        let holder_id = Self::holder_id();
        let ttl_secs = ttl.as_secs().max(1);
        let redis_key = self.client.scoped_key(&format!("{LOCK_KEY_PREFIX}{key}"));

        let mut conn = self.client.conn().clone();
        let acquired: Option<String> = conn
            .set_options(
                &redis_key,
                &holder_id,
                SetOptions::default()
                    .conditional_set(redis::ExistenceCheck::NX)
                    .with_expiration(SetExpiry::EX(ttl_secs)),
            )
            .await
            .map_err(|e| AppError::ServiceUnavailable(format!("Redis SET NX 失败: {}", e)))?;

        if acquired.is_some() {
            let key_owned = redis_key;
            let holder_clone = holder_id.clone();
            let client = self.client.clone();

            // 使用乐观事务释放锁，仅在持有者仍匹配时删除。
            let release_fn: Box<dyn Fn() -> AppResult<()> + Send + Sync> = Box::new(move || {
                let client = client.clone();
                let key = key_owned.clone();
                let holder = holder_clone.clone();

                // `Drop` 中无法 `await`。应在当前运行时调度比较并删除操作，且不能阻塞 Tokio 工作线程。
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        let release_key = key.clone();
                        let release_holder = holder.clone();
                        let watched_key = release_key.clone();
                        match client
                            .transaction(&[watched_key.as_str()], move |mut conn, mut pipe| {
                                let key = release_key.clone();
                                let holder = release_holder.clone();
                                async move {
                                    let current: Option<String> = conn.get(&key).await?;
                                    if current.as_deref() != Some(holder.as_str()) {
                                        return Ok(Some(0_i32));
                                    }
                                    pipe.del(&key).query_async(&mut conn).await
                                }
                            })
                            .await
                        {
                            Ok(1) => tracing::debug!("分布式锁已释放 [key={}]", key),
                            Ok(0) => {
                                tracing::debug!("分布式锁已过期或被其他实例持有 [key={}]", key)
                            }
                            Err(e) => {
                                tracing::warn!("分布式锁释放事务执行失败 [key={}]: {}", key, e)
                            }
                            _ => {}
                        }
                    });
                    Ok(())
                } else {
                    Err(AppError::Internal(format!(
                        "cannot schedule Redis lock release without a Tokio runtime: {key}"
                    )))
                }
            });

            Ok(Some(LockGuard {
                inner: Arc::new(LockGuardInner {
                    release_fn,
                    key: key.to_string(),
                    holder_id,
                    released: AtomicBool::new(false),
                }),
            }))
        } else {
            Ok(None)
        }
    }

    async fn force_release(&self, key: &str) -> AppResult<bool> {
        let redis_key = format!("{LOCK_KEY_PREFIX}{key}");
        let result = self
            .client
            .del(redis_key)
            .await
            .map_err(|e| AppError::ServiceUnavailable(format!("Redis DEL 失败: {}", e)))?;
        Ok(result > 0)
    }
}

#[derive(Clone)]
struct LocalLockEntry {
    holder_id: String,
    expires_at: Instant,
}

/// 仅用于显式单实例开发回退的进程内锁。与 `NoopLock` 不同，它仍可保证互斥。
#[derive(Clone, Default)]
pub struct LocalDistributedLock {
    entries: Arc<DashMap<String, LocalLockEntry>>,
}

impl LocalDistributedLock {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl DistributedLock for LocalDistributedLock {
    async fn try_acquire(&self, key: &str, ttl: Duration) -> AppResult<Option<LockGuard>> {
        let holder_id = RedisDistributedLock::holder_id();
        let expires_at = Instant::now() + ttl.max(Duration::from_secs(1));
        match self.entries.entry(key.to_owned()) {
            Entry::Vacant(entry) => {
                entry.insert(LocalLockEntry {
                    holder_id: holder_id.clone(),
                    expires_at,
                });
            }
            Entry::Occupied(mut entry) if entry.get().expires_at <= Instant::now() => {
                entry.insert(LocalLockEntry {
                    holder_id: holder_id.clone(),
                    expires_at,
                });
            }
            Entry::Occupied(_) => return Ok(None),
        }

        let entries = self.entries.clone();
        let key_owned = key.to_owned();
        let holder = holder_id.clone();
        Ok(Some(LockGuard {
            inner: Arc::new(LockGuardInner {
                release_fn: Box::new(move || {
                    let owned = entries
                        .get(&key_owned)
                        .is_some_and(|entry| entry.holder_id == holder);
                    if owned {
                        entries.remove(&key_owned);
                    }
                    Ok(())
                }),
                key: key.to_owned(),
                holder_id,
                released: AtomicBool::new(false),
            }),
        }))
    }

    async fn force_release(&self, key: &str) -> AppResult<bool> {
        Ok(self.entries.remove(key).is_some())
    }
}

/// 创建分布式锁实例
///
/// - 配置了 Redis → 使用 `RedisDistributedLock`
/// - 未配置 Redis → 使用真正互斥的进程内锁（显式单实例降级）
pub fn create_distributed_lock(redis: Option<&RedisClient>) -> Arc<dyn DistributedLock> {
    match redis {
        Some(client) => {
            tracing::info!("分布式锁: Redis 模式");
            Arc::new(RedisDistributedLock::new(client.clone()))
        }
        None => {
            tracing::warn!("分布式锁: 进程内降级模式（仅支持单实例部署）");
            Arc::new(LocalDistributedLock::new())
        }
    }
}
