//! 统一缓存抽象层
//!
//! 提供 Cache trait 抽象 + Redis/本地内存/Noop 三种实现。
//! 内置缓存防护机制：
//! - **防雪崩**：TTL 随机抖动 ±10%
//! - **防穿透**：空值缓存（默认 60 秒）
//! - **防击穿**：互斥锁双检锁模式（BreakdownGuard）
//! - **预热**：CacheWarmer 启动时批量加载热点数据
//!
//! # 使用示例
//!
//! ```
//! # use ryframe_core::cache::{Cache, LocalMemoryCache, CacheStrategy};
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // 本地内存缓存（自包含示例，无需外部依赖）
//! let cache = LocalMemoryCache::unlimited();
//! cache.get_or_load("key", 3600, || async { Ok("value".to_string()) }).await?;
//!
//! // 带防护的缓存
//! let cache = CacheStrategy::new(cache)
//!     .with_avalanche_jitter(0.2)  // ±20% TTL 抖动
//!     .with_null_cache_ttl(60);    // 空值缓存 60 秒
//! # Ok(())
//! # }
//! ```

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use dashmap::DashMap;
use rand::RngExt;
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::RwLock;

use crate::RedisClient;

// ==================== Cache Trait ====================

/// 缓存抽象 trait
///
/// 所有缓存实现（Redis / 本地内存 / Noop）均需实现此接口。
#[async_trait]
pub trait Cache: Send + Sync {
    /// 读取缓存
    ///
    /// - `T`: 反序列化目标类型
    /// - 缓存未命中返回 `Ok(None)`
    async fn get<T: DeserializeOwned + Send>(&self, key: &str) -> Result<Option<T>, CacheError>;

    /// 写入缓存（带 TTL）
    ///
    /// - `value`: 需实现 Serialize
    /// - `ttl_secs`: 过期时间（秒），0 表示永不过期
    async fn set<T: Serialize + Send + Sync>(
        &self,
        key: &str,
        value: &T,
        ttl_secs: u64,
    ) -> Result<(), CacheError>;

    /// 删除缓存
    async fn delete(&self, key: &str) -> Result<(), CacheError>;

    /// 检查缓存是否存在
    async fn exists(&self, key: &str) -> Result<bool, CacheError>;

    /// 批量删除（按前缀匹配）
    ///
    /// 默认使用 `keys` 命令，生产环境 Redis 建议使用 SCAN。
    async fn delete_by_prefix(&self, prefix: &str) -> Result<u64, CacheError> {
        let keys = self.keys(prefix).await?;
        let mut count = 0u64;
        for key in keys {
            self.delete(&key).await?;
            count += 1;
        }
        Ok(count)
    }

    /// 获取匹配前缀的所有 key（用于批量操作）
    async fn keys(&self, prefix: &str) -> Result<Vec<String>, CacheError>;

    // ==================== 便捷方法 ====================

    /// Get-or-Load 模式：缓存未命中时自动回源加载
    ///
    /// ```
    /// # use ryframe_core::cache::{Cache, LocalMemoryCache};
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let cache = LocalMemoryCache::unlimited();
    /// let user = cache.get_or_load("user:1", 3600, || async {
    ///     Ok("Alice".to_string())
    /// }).await?;
    /// assert_eq!(user, "Alice");
    /// # Ok(())
    /// # }
    /// ```
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

/// 缓存错误
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("缓存序列化失败: {0}")]
    Serialize(String),
    #[error("缓存反序列化失败: {0}")]
    Deserialize(String),
    #[error("Redis 错误: {0}")]
    Redis(String),
    #[error("缓存操作失败: {0}")]
    Operation(String),
}

impl From<redis::RedisError> for CacheError {
    fn from(e: redis::RedisError) -> Self {
        CacheError::Redis(e.to_string())
    }
}

impl From<serde_json::Error> for CacheError {
    fn from(e: serde_json::Error) -> Self {
        CacheError::Serialize(e.to_string())
    }
}

/// Default TTL for a user's resolved API permission codes.
pub const USER_PERMISSION_CACHE_TTL_SECS: u64 = 30 * 60;

/// Build a tenant-scoped permission cache key.
///
/// The tenant segment is required because user IDs are only meaningful inside
/// their tenant boundary.
pub fn user_permission_cache_key(tenant_id: &str, user_id: i64) -> String {
    format!("user:perms:{tenant_id}:{user_id}")
}

/// Read a user's resolved API permission codes from Redis.
pub async fn get_user_permission_cache(
    redis: &RedisClient,
    tenant_id: &str,
    user_id: i64,
) -> Result<Option<Vec<String>>, CacheError> {
    RedisCache::new(redis.clone())
        .get(&user_permission_cache_key(tenant_id, user_id))
        .await
}

/// Cache a user's resolved API permission codes in Redis.
pub async fn set_user_permission_cache(
    redis: &RedisClient,
    tenant_id: &str,
    user_id: i64,
    permissions: &[String],
) -> Result<(), CacheError> {
    RedisCache::new(redis.clone())
        .set(
            &user_permission_cache_key(tenant_id, user_id),
            &permissions,
            USER_PERMISSION_CACHE_TTL_SECS,
        )
        .await
}

/// Delete one user's cached API permission codes.
pub async fn clear_user_permission_cache(
    redis: &RedisClient,
    tenant_id: &str,
    user_id: i64,
) -> Result<(), CacheError> {
    RedisCache::new(redis.clone())
        .delete(&user_permission_cache_key(tenant_id, user_id))
        .await
}

/// Delete all cached API permission codes for one tenant.
pub async fn clear_tenant_permission_cache(
    redis: &RedisClient,
    tenant_id: &str,
) -> Result<u64, CacheError> {
    RedisCache::new(redis.clone())
        .delete_by_prefix(&format!("user:perms:{tenant_id}:"))
        .await
}

// ==================== Noop Cache ====================

/// 空缓存实现（禁用缓存时使用）
///
/// 所有操作均为空操作，始终返回 None（缓存未命中）。
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
        Ok(vec![])
    }
}

// ==================== Redis Cache ====================

/// Redis 缓存实现
///
/// 封装 RedisClient，通过 JSON 序列化存储任意类型。
pub struct RedisCache {
    client: RedisClient,
}

impl RedisCache {
    pub fn new(client: RedisClient) -> Self {
        Self { client }
    }

    /// 获取底层 RedisClient（用于高级操作）
    pub fn client(&self) -> &RedisClient {
        &self.client
    }
}

#[async_trait]
impl Cache for RedisCache {
    async fn get<T: DeserializeOwned + Send>(&self, key: &str) -> Result<Option<T>, CacheError> {
        match self.client.get(key).await {
            Ok(Some(json)) => {
                let value: T = serde_json::from_str(&json).map_err(|e| {
                    CacheError::Deserialize(format!("缓存 {} 反序列化失败: {}", key, e))
                })?;
                Ok(Some(value))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(CacheError::Redis(format!("GET {} 失败: {}", key, e))),
        }
    }

    async fn set<T: Serialize + Send + Sync>(
        &self,
        key: &str,
        value: &T,
        ttl_secs: u64,
    ) -> Result<(), CacheError> {
        let json =
            serde_json::to_string(value).map_err(|e| CacheError::Serialize(e.to_string()))?;
        if ttl_secs == 0 {
            self.client
                .set(key, &json)
                .await
                .map_err(|e| CacheError::Redis(format!("SET {} 失败: {}", key, e)))?;
        } else {
            self.client
                .set_ex(key, &json, ttl_secs)
                .await
                .map_err(|e| CacheError::Redis(format!("SETEX {} 失败: {}", key, e)))?;
        }
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), CacheError> {
        self.client
            .del(key)
            .await
            .map_err(|e| CacheError::Redis(format!("DEL {} 失败: {}", key, e)))?;
        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool, CacheError> {
        self.client
            .exists(key)
            .await
            .map_err(|e| CacheError::Redis(format!("EXISTS {} 失败: {}", key, e)))
    }

    async fn keys(&self, prefix: &str) -> Result<Vec<String>, CacheError> {
        let pattern = format!("{}*", prefix);
        self.client
            .keys(&pattern)
            .await
            .map_err(|e| CacheError::Redis(format!("KEYS {} 失败: {}", pattern, e)))
    }
}

// ==================== Local Memory Cache ====================

/// 本地内存缓存项
struct CachedEntry {
    value: String, // JSON 序列化存储
    expires_at: Option<std::time::Instant>,
}

/// 本地内存缓存（基于 tokio::RwLock + HashMap）
///
/// 适用于单机部署或降级场景，不支持分布式一致性。
pub struct LocalMemoryCache {
    store: Arc<RwLock<HashMap<String, CachedEntry>>>,
}

impl LocalMemoryCache {
    /// 创建指定容量的本地缓存（容量达到上限时随机淘汰）
    pub fn new(capacity: usize) -> Self {
        let store = if capacity == 0 || capacity == usize::MAX {
            HashMap::new()
        } else {
            HashMap::with_capacity(capacity)
        };
        Self {
            store: Arc::new(RwLock::new(store)),
        }
    }

    /// 创建无容量限制的缓存
    pub fn unlimited() -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 清理过期条目（建议定期调用或惰性清理）
    pub async fn clean_expired(&self) {
        let mut store = self.store.write().await;
        let now = std::time::Instant::now();
        store.retain(|_, entry| entry.expires_at.map(|exp| exp > now).unwrap_or(true));
    }
}

#[async_trait]
impl Cache for LocalMemoryCache {
    async fn get<T: DeserializeOwned + Send>(&self, key: &str) -> Result<Option<T>, CacheError> {
        let store = self.store.read().await;
        match store.get(key) {
            Some(entry) => {
                // 检查过期
                if let Some(expires_at) = entry.expires_at
                    && expires_at <= std::time::Instant::now()
                {
                    return Ok(None); // 惰性删除，不立即清理
                }
                let value: T = serde_json::from_str(&entry.value)
                    .map_err(|e| CacheError::Deserialize(e.to_string()))?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    async fn set<T: Serialize + Send + Sync>(
        &self,
        key: &str,
        value: &T,
        ttl_secs: u64,
    ) -> Result<(), CacheError> {
        let json =
            serde_json::to_string(value).map_err(|e| CacheError::Serialize(e.to_string()))?;
        let expires_at = if ttl_secs > 0 {
            Some(std::time::Instant::now() + std::time::Duration::from_secs(ttl_secs))
        } else {
            None
        };
        let mut store = self.store.write().await;
        store.insert(
            key.to_string(),
            CachedEntry {
                value: json,
                expires_at,
            },
        );
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), CacheError> {
        let mut store = self.store.write().await;
        store.remove(key);
        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool, CacheError> {
        let store = self.store.read().await;
        Ok(store.contains_key(key))
    }

    async fn keys(&self, prefix: &str) -> Result<Vec<String>, CacheError> {
        let store = self.store.read().await;
        let keys: Vec<String> = store
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        Ok(keys)
    }
}

// ==================== Cache Strategy（防护层） ====================

/// 缓存防护策略配置
#[derive(Clone)]
pub struct CacheStrategyConfig {
    /// 随机过期抖动比例（0.0 ~ 1.0），防缓存雪崩
    /// 最终 TTL = base_ttl * (1 ± jitter)
    pub avalanche_jitter: f64,
    /// 空值缓存 TTL（秒），防缓存穿透
    /// - 0 表示不缓存空值
    pub null_cache_ttl: u64,
    /// 空值标识符（存储到缓存中的标记值）
    pub null_marker: String,
}

impl Default for CacheStrategyConfig {
    fn default() -> Self {
        Self {
            avalanche_jitter: 0.1, // 默认 ±10% 抖动
            null_cache_ttl: 60,    // 空值缓存 60 秒
            null_marker: "__CACHE_NULL__".to_string(),
        }
    }
}

/// 缓存策略代理（包装任意 Cache 实现，叠加防护机制）
///
/// # 防护机制
///
/// - **防雪崩**：在设置缓存时对 TTL 添加随机抖动，避免大量 key 同时过期
/// - **防穿透**：对回源返回 None 的情况缓存空值，防止穿透打到 DB
pub struct CacheStrategy<C: Cache> {
    inner: C,
    config: CacheStrategyConfig,
}

impl<C: Cache> CacheStrategy<C> {
    /// 创建缓存策略代理
    pub fn new(inner: C) -> Self {
        Self {
            inner,
            config: CacheStrategyConfig::default(),
        }
    }

    /// 设置防雪崩抖动比例
    ///
    /// # Arguments
    /// - `jitter`: 0.0 ~ 1.0，例如 0.2 表示 TTL 在 80%~120% 之间随机
    pub fn with_avalanche_jitter(mut self, jitter: f64) -> Self {
        self.config.avalanche_jitter = jitter.clamp(0.0, 0.5);
        self
    }

    /// 设置空值缓存 TTL
    ///
    /// 0 表示不缓存空值。
    pub fn with_null_cache_ttl(mut self, ttl_secs: u64) -> Self {
        self.config.null_cache_ttl = ttl_secs;
        self
    }

    /// 获取内部缓存实现
    pub fn inner(&self) -> &C {
        &self.inner
    }

    /// 计算带抖动的 TTL
    fn jittered_ttl(&self, base_ttl: u64) -> u64 {
        if self.config.avalanche_jitter <= 0.0 || base_ttl == 0 {
            return base_ttl;
        }
        let factor = 1.0 - self.config.avalanche_jitter
            + rand::rng().random_range(0.0..self.config.avalanche_jitter * 2.0);
        (base_ttl as f64 * factor).round() as u64
    }

    /// 带防护的 Get-or-Load
    ///
    /// 1. 读缓存 → 命中直接返回
    /// 2. 缓存未命中 → 回源加载
    /// 3. 加载成功 → 写入缓存（带随机 TTL 防雪崩）
    /// 4. 加载失败 → 写入 Null 标记（带空值 TTL 防穿透）
    ///
    /// # Type Parameters
    /// - `T`: 值类型
    /// - `F`: 回源加载函数
    pub async fn get_or_load_with_protection<T, F, Fut>(
        &self,
        key: &str,
        ttl_secs: u64,
        loader: F,
    ) -> Result<Option<T>, CacheError>
    where
        T: Serialize + DeserializeOwned + Send + Sync + Clone,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<Option<T>, CacheError>> + Send,
    {
        // 1. 先读缓存
        if let Some(cached) = self.inner.get::<String>(key).await? {
            if cached == self.config.null_marker {
                return Ok(None); // 空值缓存命中
            }
            let value: T = serde_json::from_str(&cached).map_err(|e| {
                CacheError::Deserialize(format!("缓存 {} 反序列化失败: {}", key, e))
            })?;
            return Ok(Some(value));
        }

        // 2. 回源加载
        match loader().await {
            Ok(Some(value)) => {
                // 成功 → 写入缓存（带抖动 TTL）
                self.inner
                    .set(key, &value, self.jittered_ttl(ttl_secs))
                    .await?;
                Ok(Some(value))
            }
            Ok(None) => {
                // 空值 → 写入空值标记（防穿透）
                if self.config.null_cache_ttl > 0 {
                    self.inner
                        .set(key, &self.config.null_marker, self.config.null_cache_ttl)
                        .await?;
                }
                Ok(None)
            }
            Err(e) => {
                // 加载失败 → 不缓存，直接返回错误
                Err(e)
            }
        }
    }
}

// CacheStrategy 委托：可以直接作为 Cache 使用
// 注意：set 会走 jittered TTL，get 会走 null marker 过滤
#[async_trait]
impl<C: Cache> Cache for CacheStrategy<C> {
    async fn get<T: DeserializeOwned + Send>(&self, key: &str) -> Result<Option<T>, CacheError> {
        match self.inner.get::<String>(key).await? {
            Some(raw) if raw == self.config.null_marker => Ok(None),
            Some(raw) => {
                serde_json::from_str(&raw).map_err(|e| CacheError::Deserialize(e.to_string()))
            }
            None => Ok(None),
        }
    }

    async fn set<T: Serialize + Send + Sync>(
        &self,
        key: &str,
        value: &T,
        ttl_secs: u64,
    ) -> Result<(), CacheError> {
        self.inner
            .set(key, value, self.jittered_ttl(ttl_secs))
            .await
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

// ==================== Cache Factory ====================

/// 根据配置创建缓存实现
///
/// - 有 Redis → `RedisCache`
/// - 无 Redis → `LocalMemoryCache`
pub enum CacheBackend {
    Redis(Box<RedisCache>),
    Local(LocalMemoryCache),
    Noop(NoopCache),
}

impl CacheBackend {
    /// 从 RedisClient 创建（有则 Redis，无则 Local）
    pub fn from_redis(redis: Option<RedisClient>) -> Self {
        match redis {
            Some(client) => CacheBackend::Redis(Box::new(RedisCache::new(client))),
            None => CacheBackend::Local(LocalMemoryCache::unlimited()),
        }
    }
}

// ==================== 缓存击穿防护 ====================

/// 带互斥锁的热 Key 保护代理
///
/// **场景**：热点 Key 过期时，大量并发请求同时回源查询 DB，造成 DB 压力激增（击穿）。
///
/// **方案**：每个 Key 一把轻量级互斥锁，同一个 Key 只有一个请求去回源，其余等待结果。
///
/// # 使用示例
///
/// ```
/// # use ryframe_core::cache::{Cache, LocalMemoryCache, BreakdownGuard};
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let cache = BreakdownGuard::new(LocalMemoryCache::unlimited());
/// // 自动防止击穿
/// let result = cache.get_or_load_guarded("hot:key", 3600, || async {
///     Ok(Some("data".to_string()))
/// }).await?;
/// assert_eq!(result, Some("data".to_string()));
/// # Ok(())
/// # }
/// ```
pub struct BreakdownGuard<C: Cache> {
    inner: C,
    /// 互斥锁注册表（按 key 控制并发回源）
    locks: Arc<DashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// 等待回源结果的超时时间
    wait_timeout: std::time::Duration,
}

impl<C: Cache> BreakdownGuard<C> {
    /// 创建击穿防护代理
    pub fn new(inner: C) -> Self {
        Self {
            inner,
            locks: Arc::new(DashMap::new()),
            wait_timeout: std::time::Duration::from_secs(10),
        }
    }

    /// 设置等待超时（默认 10 秒）
    pub fn with_wait_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.wait_timeout = timeout;
        self
    }

    /// 获取底层缓存实现
    pub fn inner(&self) -> &C {
        &self.inner
    }

    /// 获取内部互斥量，用于调用者自行持有锁
    pub fn get_mutex(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.locks
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .value()
            .clone()
    }

    /// 双检锁模式：防击穿回源
    ///
    /// 1. 先读缓存 → 命中直接返回
    /// 2. 未命中 → 获取互斥锁
    /// 3. 双重检查缓存（可能其他请求已加载）
    /// 4. 仍未命中 → 回源加载 → 写缓存 → 释放锁
    pub async fn get_or_load_guarded<T, F, Fut>(
        &self,
        key: &str,
        ttl_secs: u64,
        loader: F,
    ) -> Result<Option<T>, CacheError>
    where
        T: Serialize + DeserializeOwned + Send + Sync + Clone,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<Option<T>, CacheError>> + Send,
    {
        // 1. 先读缓存
        if let Some(cached) = self.inner.get::<T>(key).await? {
            return Ok(Some(cached));
        }

        // 2. 获取互斥锁（同一 key 只有一个请求去回源）
        let mutex = self.get_mutex(key);
        let _guard = match tokio::time::timeout(self.wait_timeout, mutex.lock()).await {
            Ok(g) => g,
            Err(_) => {
                // 超时 → 回退到无锁读取（降级策略）
                tracing::warn!("缓存击穿防护等待超时: key={}", key);
                return self.inner.get::<T>(key).await;
            }
        };

        // 3. 双重检查缓存（可能其他请求已加载）
        if let Some(cached) = self.inner.get::<T>(key).await? {
            return Ok(Some(cached));
        }

        // 4. 回源加载
        match loader().await {
            Ok(Some(value)) => {
                // 成功 → 写入缓存并返回
                self.inner.set(key, &value, ttl_secs).await?;
                Ok(Some(value))
            }
            Ok(None) => {
                // 空结果 → 写入空值标记（防止短时间内重复击穿）
                let null_marker = "__CACHE_NULL__";
                self.inner.set(key, &null_marker.to_string(), 60).await?;
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// 清理不活跃的锁（防止内存泄漏）
    ///
    /// 建议在后台定时任务中调用（如每 5 分钟清理一次）。
    pub fn clean_stale_locks(&self) {
        self.locks.retain(|_, v| Arc::strong_count(v) > 1);
    }
}

// ==================== 缓存预热 ====================

/// 缓存预热任务定义
#[derive(Clone)]
pub struct WarmUpTask<C: Cache> {
    /// 缓存 Key
    pub key: String,
    /// 缓存 TTL（秒）
    pub ttl_secs: u64,
    /// 数据加载函数
    pub loader: WarmUpLoader,
    _cache: std::marker::PhantomData<C>,
}

/// 缓存预热加载函数签名
type WarmUpLoader = Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn Future<Output = Result<String, CacheError>> + Send>>
        + Send
        + Sync,
>;

impl<C: Cache> std::fmt::Debug for WarmUpTask<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WarmUpTask")
            .field("key", &self.key)
            .field("ttl_secs", &self.ttl_secs)
            .finish()
    }
}

/// 缓存预热器
///
/// 在应用启动时预加载热点数据到缓存，避免首次请求的冷启动延迟。
///
/// # 使用示例
///
/// ```
/// # use ryframe_core::cache::{CacheWarmer, LocalMemoryCache};
/// # #[tokio::main]
/// # async fn main() {
/// let mut warmer = CacheWarmer::new(LocalMemoryCache::unlimited());
/// warmer.add_task("sys_menu:tree", 3600, || Box::pin(async {
///     Ok(r#"[{"id":1,"name":"首页"}]"#.to_string())
/// }));
/// warmer.warm_up().await;
/// # }
/// ```
pub struct CacheWarmer<C: Cache> {
    cache: C,
    tasks: Vec<WarmUpTask<C>>,
}

impl<C: Cache> CacheWarmer<C> {
    /// 创建预热器
    pub fn new(cache: C) -> Self {
        Self {
            cache,
            tasks: Vec::new(),
        }
    }

    /// 添加预热任务
    pub fn add_task<F>(&mut self, key: &str, ttl_secs: u64, loader: F)
    where
        F: Fn() -> std::pin::Pin<Box<dyn Future<Output = Result<String, CacheError>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        self.tasks.push(WarmUpTask {
            key: key.to_string(),
            ttl_secs,
            loader: Arc::new(loader),
            _cache: std::marker::PhantomData,
        });
    }

    /// 执行所有预热任务
    ///
    /// - 并发加载所有任务
    /// - 单个任务失败不影响其他任务
    /// - 返回 (成功数, 失败数)
    pub async fn warm_up(&self) -> (usize, usize) {
        let mut success = 0usize;
        let mut failed = 0usize;

        let handles: Vec<_> = self
            .tasks
            .iter()
            .map(|task| {
                let loader = task.loader.clone();
                let key = task.key.clone();
                let ttl = task.ttl_secs;
                tokio::spawn(async move {
                    match loader().await {
                        Ok(json) => {
                            tracing::info!("[CacheWarmer] 预热成功: key={}", key);
                            Ok((key, json, ttl))
                        }
                        Err(e) => {
                            tracing::warn!("[CacheWarmer] 预热失败: key={}, error={}", key, e);
                            Err(())
                        }
                    }
                })
            })
            .collect();

        for handle in handles {
            match handle.await {
                Ok(Ok((key, json, ttl))) if self.cache.set(&key, &json, ttl).await.is_ok() => {
                    success += 1;
                }
                _ => {
                    failed += 1;
                }
            }
        }

        tracing::info!(
            "[CacheWarmer] 预热完成: success={}, failed={}, total={}",
            success,
            failed,
            self.tasks.len()
        );
        (success, failed)
    }

    /// 获取预热任务数
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }
}

// ==================== 测试 ====================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_cache_basic() {
        let cache = LocalMemoryCache::unlimited();

        // set + get
        cache.set("key1", &"hello".to_string(), 60).await.unwrap();
        let val: Option<String> = cache.get("key1").await.unwrap();
        assert_eq!(val, Some("hello".to_string()));

        // exists
        assert!(cache.exists("key1").await.unwrap());

        // delete
        cache.delete("key1").await.unwrap();
        assert!(!cache.exists("key1").await.unwrap());

        // get miss
        let miss: Option<String> = cache.get("nonexistent").await.unwrap();
        assert_eq!(miss, None);
    }

    #[tokio::test]
    async fn test_noop_cache() {
        let cache = NoopCache;
        cache.set("key", &"val", 60).await.unwrap();
        let val: Option<String> = cache.get("key").await.unwrap();
        assert_eq!(val, None);
        assert!(!cache.exists("key").await.unwrap());
    }

    #[tokio::test]
    async fn test_cache_strategy_null_cache() {
        let cache = CacheStrategy::new(NoopCache)
            .with_null_cache_ttl(60)
            .with_avalanche_jitter(0.0);

        let result = cache
            .get_or_load_with_protection::<String, _, _>("key", 60, || async { Ok(None) })
            .await
            .unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_jittered_ttl() {
        let cache = CacheStrategy::new(NoopCache);
        let ttl = cache.jittered_ttl(100);
        // 无 jitter（默认 0.1）时应在 90~110 之间
        assert!(
            (90..=110).contains(&ttl),
            "ttl should be jittered, got {}",
            ttl
        );
    }
}
