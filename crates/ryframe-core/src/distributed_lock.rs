//! 分布式锁
//!
//! 基于 Redis SET NX 实现分布式锁，用于多实例场景下的资源互斥访问。
//! 支持 RAII 模式自动释放，防止死锁。
//!
//! # 使用示例
//!
//! ```rust,ignore
//! use ryframe_core::distributed_lock::{DistributedLock, RedisDistributedLock};
//! use std::time::Duration;
//!
//! let lock = RedisDistributedLock::new(redis_client);
//! if let Some(guard) = lock.try_acquire("my_resource", Duration::from_secs(30)).await? {
//!     // 获取到锁，执行临界区代码
//!     do_critical_work().await;
//!     // guard 在 drop 时自动释放锁
//! }
//! ```

use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ryframe_common::{AppError, AppResult};

use crate::redis_client::RedisClient;

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
/// ```rust,ignore
/// {
///     let guard = lock.try_acquire("task:clean_log", ttl).await?.unwrap();
///     // 执行临界区代码
///     // guard 离开作用域时自动释放锁
/// }
/// ```
#[derive(Debug)]
pub struct LockGuard {
    inner: Arc<LockGuardInner>,
}

struct LockGuardInner {
    release_fn: Box<dyn Fn() -> AppResult<()> + Send + Sync>,
    key: String,
    holder_id: String,
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
        (self.inner.release_fn)()
    }
}

impl Drop for LockGuardInner {
    fn drop(&mut self) {
        if let Err(e) = (self.release_fn)() {
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
        format!("{}-{}", timestamp, pid)
    }
}

#[async_trait::async_trait]
impl DistributedLock for RedisDistributedLock {
    async fn try_acquire(&self, key: &str, ttl: Duration) -> AppResult<Option<LockGuard>> {
        let holder_id = Self::holder_id();
        let ttl_secs = ttl.as_secs().max(1);

        // 使用 SET NX 原子获取锁
        let script = redis::Script::new(
            r"
            if redis.call('SET', KEYS[1], ARGV[1], 'NX', 'EX', ARGV[2]) then
                return 1
            else
                return 0
            end
        ",
        );

        let mut conn = self.client.conn().clone();
        let result: i32 = script
            .key(key)
            .arg(&holder_id)
            .arg(ttl_secs)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| AppError::Internal(format!("Redis SET NX 失败: {}", e)))?;

        if result == 1 {
            let key_owned = key.to_string();
            let holder_clone = holder_id.clone();
            let client = self.client.clone();

            // 使用 Lua 脚本释放锁（仅当持有者匹配时才释放）
            let release_fn: Box<dyn Fn() -> AppResult<()> + Send + Sync> = Box::new(move || {
                let client = client.clone();
                let key = key_owned.clone();
                let holder = holder_clone.clone();

                // 同步释放（在 drop 中无法使用 async）
                let rt = tokio::runtime::Handle::try_current();
                if let Ok(handle) = rt {
                    handle.block_on(async move {
                        let release_script = redis::Script::new(
                            r"
                            if redis.call('GET', KEYS[1]) == ARGV[1] then
                                return redis.call('DEL', KEYS[1])
                            else
                                return 0
                            end
                        ",
                        );
                        let mut conn = client.conn().clone();
                        match release_script
                            .key(key.as_str())
                            .arg(&holder)
                            .invoke_async::<i32>(&mut conn)
                            .await
                        {
                            Ok(1) => tracing::debug!("分布式锁已释放 [key={}]", key),
                            Ok(0) => {
                                tracing::debug!("分布式锁已过期或被其他实例持有 [key={}]", key)
                            }
                            Err(e) => {
                                tracing::warn!("分布式锁释放脚本执行失败 [key={}]: {}", key, e)
                            }
                            _ => {}
                        }
                        Ok(())
                    })
                } else {
                    // 没有 tokio runtime（如在 sync context），放弃释放
                    tracing::warn!("无法获取 Tokio Runtime，分布式锁可能泄漏 [key={}]", key);
                    Ok(())
                }
            });

            Ok(Some(LockGuard {
                inner: Arc::new(LockGuardInner {
                    release_fn,
                    key: key.to_string(),
                    holder_id,
                }),
            }))
        } else {
            Ok(None)
        }
    }

    async fn force_release(&self, key: &str) -> AppResult<bool> {
        let result = self
            .client
            .del(key)
            .await
            .map_err(|e| AppError::Internal(format!("Redis DEL 失败: {}", e)))?;
        Ok(result > 0)
    }
}

/// 空分布式锁实现（单实例模式）
///
/// 始终返回成功，适用于单实例部署或开发环境。
#[derive(Clone, Default)]
pub struct NoopLock;

impl NoopLock {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl DistributedLock for NoopLock {
    async fn try_acquire(&self, key: &str, _ttl: Duration) -> AppResult<Option<LockGuard>> {
        tracing::debug!("NoopLock: 跳过分布式锁获取 [key={}]", key);
        Ok(Some(LockGuard {
            inner: Arc::new(LockGuardInner {
                release_fn: Box::new(|| Ok(())),
                key: key.to_string(),
                holder_id: "noop".to_string(),
            }),
        }))
    }

    async fn force_release(&self, _key: &str) -> AppResult<bool> {
        Ok(true)
    }
}

/// 创建分布式锁实例
///
/// - 配置了 Redis → 使用 `RedisDistributedLock`
/// - 未配置 Redis → 使用 `NoopLock`（单实例模式，永远获取成功）
pub fn create_distributed_lock(redis: Option<&RedisClient>) -> Arc<dyn DistributedLock> {
    match redis {
        Some(client) => {
            tracing::info!("分布式锁: Redis 模式");
            Arc::new(RedisDistributedLock::new(client.clone()))
        }
        None => {
            tracing::info!("分布式锁: Noop 模式（单实例部署）");
            Arc::new(NoopLock::new())
        }
    }
}
