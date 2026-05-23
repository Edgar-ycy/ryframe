use axum::{
    extract::ConnectInfo,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use dashmap::DashMap;
use ryframe_core::RedisClient;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Redis key 前缀
const RATE_LIMIT_KEY_PREFIX: &str = "rate_limit:";

/// 令牌桶（仅内存模式使用）
pub(crate) struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

/// 限流器（支持 Redis / 内存双模式）
///
/// - Redis 模式：使用 Redis 计数器 + 固定窗口，支持分布式部署
/// - 内存模式：DashMap 令牌桶，单机限流
#[derive(Clone)]
#[allow(private_interfaces)]
pub enum RateLimiter {
    /// Redis 限流（生产推荐）
    Redis {
        client: Box<RedisClient>,
        capacity: u32,
        /// 窗口时长（秒）
        window_secs: u64,
    },
    /// 内存限流（开发/降级模式）
    InMemory { inner: Arc<RateLimiterInner> },
}

pub(crate) struct RateLimiterInner {
    buckets: DashMap<String, Bucket>,
    capacity: f64,
    refill_per_sec: f64,
}

impl RateLimiter {
    /// 创建 Redis 模式的限流器
    ///
    /// `window_secs`：固定窗口时长（秒），每个窗口内最多 `capacity` 次请求
    pub fn new_redis(client: RedisClient, capacity: u32, window_secs: u64) -> Self {
        Self::Redis {
            client: Box::new(client),
            capacity,
            window_secs,
        }
    }

    /// 创建内存模式的限流器
    pub fn new_in_memory(capacity: u32, refill_per_sec: u32) -> Self {
        Self::InMemory {
            inner: Arc::new(RateLimiterInner {
                buckets: DashMap::new(),
                capacity: capacity as f64,
                refill_per_sec: refill_per_sec as f64,
            }),
        }
    }

    /// 兼容旧 API
    pub fn new(capacity: u32, refill_per_sec: u32) -> Self {
        Self::new_in_memory(capacity, refill_per_sec)
    }

    /// 尝试获取 1 个令牌，返回是否通过
    pub async fn try_acquire(&self, key: &str) -> bool {
        match self {
            Self::Redis {
                client,
                capacity,
                window_secs,
            } => {
                let redis_key = format!("{}{}", RATE_LIMIT_KEY_PREFIX, key);
                // 使用 INCR + EXPIRE 实现固定窗口限流
                match client.incr(&redis_key).await {
                    Ok(count) => {
                        // 首次设置过期时间
                        if count == 1 {
                            let _ = client.expire(&redis_key, *window_secs).await;
                        }
                        count <= *capacity as i64
                    }
                    Err(e) => {
                        tracing::error!("Redis INCR 限流失败，放行请求: {}", e);
                        true // Redis 故障时放行，避免阻断业务
                    }
                }
            }
            Self::InMemory { inner } => {
                let now = Instant::now();
                let mut bucket = inner.buckets.entry(key.to_string()).or_insert(Bucket {
                    tokens: inner.capacity,
                    last_refill: now,
                });

                let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
                bucket.tokens =
                    (bucket.tokens + elapsed * inner.refill_per_sec).min(inner.capacity);
                bucket.last_refill = now;

                if bucket.tokens >= 1.0 {
                    bucket.tokens -= 1.0;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// 获取当前可用令牌数（仅内存模式，用于测试）
    #[cfg(test)]
    pub fn available_tokens(&self, key: &str) -> f64 {
        match self {
            Self::InMemory { inner } => inner
                .buckets
                .get(key)
                .map(|b| b.tokens)
                .unwrap_or(inner.capacity),
            Self::Redis { capacity, .. } => *capacity as f64,
        }
    }

    /// 后台清理（仅内存模式有效）
    pub fn spawn_gc(self: &Arc<Self>) {
        if let Self::InMemory { inner } = self.as_ref() {
            let inner = inner.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
                loop {
                    interval.tick().await;
                    let cutoff = Instant::now() - Duration::from_secs(300);
                    inner.buckets.retain(|_, b| b.last_refill > cutoff);
                }
            });
        }
        // Redis 模式由 Redis 自身 EXPIRE 机制管理，无需后台 GC
    }
}

/// Axum 限流中间件
pub async fn rate_limit_middleware(
    axum::extract::State(limiter): axum::extract::State<Arc<RateLimiter>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: axum::extract::Request,
    next: Next,
) -> Result<Response, Response> {
    let key = addr.ip().to_string();
    if limiter.try_acquire(&key).await {
        Ok(next.run(req).await)
    } else {
        Err((StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded").into_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limiter() {
        // 基本限流：容量 3，前 3 次通过，第 4 次拒绝
        let limiter = RateLimiter::new_in_memory(3, 1);
        assert!(limiter.try_acquire("test").await);
        assert!(limiter.try_acquire("test").await);
        assert!(limiter.try_acquire("test").await);
        assert!(!limiter.try_acquire("test").await);

        // 不同 key 独立
        let limiter2 = RateLimiter::new_in_memory(1, 1);
        assert!(limiter2.try_acquire("a").await);
        assert!(!limiter2.try_acquire("a").await);
        assert!(limiter2.try_acquire("b").await);

        // 令牌补充
        let limiter3 = RateLimiter::new_in_memory(2, 100);
        assert!(limiter3.try_acquire("test").await);
        assert!(limiter3.try_acquire("test").await);
        assert!(!limiter3.try_acquire("test").await);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(limiter3.try_acquire("test").await);
    }

    #[tokio::test]
    async fn test_spawn_gc() {
        let limiter = Arc::new(RateLimiter::new_in_memory(10, 1));
        limiter.try_acquire("key1").await;
        limiter.spawn_gc();
    }
}
