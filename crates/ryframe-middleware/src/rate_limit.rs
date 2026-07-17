use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use dashmap::DashMap;
use ryframe_core::RedisClient;

/// Redis key 前缀
const RATE_LIMIT_KEY_PREFIX: &str = "rate_limit:";

/// 令牌桶（仅内存模式使用）
struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

/// 限流器（支持 Redis / 内存双模式）
///
/// - Redis 模式：使用 Redis 计数器 + 固定窗口，支持分布式部署
/// - 内存模式：DashMap 令牌桶，单机限流
#[derive(Clone)]
pub struct RateLimiter {
    mode: RateLimiterMode,
}

#[derive(Clone)]
enum RateLimiterMode {
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

struct RateLimiterInner {
    buckets: DashMap<String, Bucket>,
    capacity: f64,
    refill_per_sec: f64,
}

impl RateLimiter {
    /// 创建 Redis 模式的限流器
    ///
    /// `window_secs`：固定窗口时长（秒），每个窗口内最多 `capacity` 次请求
    pub fn new_redis(client: RedisClient, capacity: u32, window_secs: u64) -> Self {
        Self {
            mode: RateLimiterMode::Redis {
                client: Box::new(client),
                capacity,
                window_secs,
            },
        }
    }

    /// 创建内存模式的限流器
    pub fn new_in_memory(capacity: u32, refill_per_sec: u32) -> Self {
        Self {
            mode: RateLimiterMode::InMemory {
                inner: Arc::new(RateLimiterInner {
                    buckets: DashMap::new(),
                    capacity: capacity as f64,
                    refill_per_sec: refill_per_sec as f64,
                }),
            },
        }
    }

    /// 尝试获取 1 个令牌，返回是否通过
    pub async fn try_acquire(&self, key: &str) -> bool {
        match &self.mode {
            RateLimiterMode::Redis {
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
            RateLimiterMode::InMemory { inner } => {
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

    /// 获取当前可用令牌数（仅内存模式）
    pub fn available_tokens(&self, key: &str) -> f64 {
        match &self.mode {
            RateLimiterMode::InMemory { inner } => inner
                .buckets
                .get(key)
                .map(|b| b.tokens)
                .unwrap_or(inner.capacity),
            RateLimiterMode::Redis { capacity, .. } => *capacity as f64,
        }
    }

    /// 后台清理（仅内存模式有效）
    pub fn spawn_gc(self: &Arc<Self>) {
        if let RateLimiterMode::InMemory { inner } = &self.mode {
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

    /// 滑动窗口限流（仅 Redis 模式）
    ///
    /// 使用 Redis Sorted Set + Lua 脚本实现原子滑动窗口。
    ///
    /// # Arguments
    /// - `key`: 限流 key
    /// - `window_secs`: 滑动窗口时长（秒）
    /// - `limit`: 窗口内最大请求数
    ///
    /// # Returns
    /// `true` 表示通过，`false` 表示限流触发
    pub async fn sliding_window_acquire(&self, key: &str, window_secs: u64, limit: u32) -> bool {
        match &self.mode {
            RateLimiterMode::Redis { client, .. } => {
                let redis_key = format!("{}sw:", RATE_LIMIT_KEY_PREFIX) + key;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs_f64();

                let script = r#"
                    local key = KEYS[1]
                    local now = tonumber(ARGV[1])
                    local window = tonumber(ARGV[2])
                    local limit = tonumber(ARGV[3])

                    redis.call('ZREMRANGEBYSCORE', key, 0, now - window)

                    local count = redis.call('ZCARD', key)
                    if count < limit then
                        redis.call('ZADD', key, now, now .. '-' .. math.random())
                        redis.call('EXPIRE', key, math.ceil(window * 2))
                        return 1
                    else
                        return 0
                    end
                "#;

                match client
                    .eval_script(
                        script,
                        &[&redis_key],
                        &[
                            &now.to_string(),
                            &window_secs.to_string(),
                            &limit.to_string(),
                        ],
                    )
                    .await
                {
                    Ok(redis::Value::Int(1)) => true,
                    Ok(redis::Value::Int(0)) => false,
                    Ok(_) => true,
                    Err(e) => {
                        tracing::error!("Redis 滑动窗口限流失败，放行请求: {}", e);
                        true
                    }
                }
            }
            RateLimiterMode::InMemory { .. } => self.try_acquire(key).await,
        }
    }

    async fn acquire_configured(
        &self,
        key: &str,
        window_secs: u64,
        limit: u32,
        use_sliding_window: bool,
    ) -> bool {
        if use_sliding_window {
            self.sliding_window_acquire(key, window_secs, limit).await
        } else {
            self.try_acquire(key).await
        }
    }

    /// 生成用户级限流 key
    pub fn user_key(user_id: &str) -> String {
        format!("user:{}", user_id)
    }

    /// 生成接口级限流 key
    pub fn api_key(path: &str) -> String {
        format!("api:{}", path)
    }

    /// 生成用户+接口级限流 key
    pub fn user_api_key(user_id: &str, path: &str) -> String {
        format!("user_api:{}:{}", user_id, path)
    }
}

/// Axum 限流中间件（IP 维度）
///
/// 根据客户端 IP 地址限流，使用令牌桶或固定窗口模式。
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
        Err((StatusCode::TOO_MANY_REQUESTS, "请求过于频繁，请稍后再试").into_response())
    }
}

/// Axum 限流状态（包含 RateLimiter + RateLimitConfig）
///
/// 用于 per-user / per-api 限流中间件。
#[derive(Clone)]
pub struct RateLimitState {
    pub limiter: Arc<RateLimiter>,
    pub config: Arc<ryframe_config::RateLimitConfig>,
}

/// 用户级限流中间件
///
/// 从 JWT Claims 中提取 user_id，对每个用户单独限流。
/// 需要在认证中间件 **之后** 注册。
///
/// 用法：
/// ```
/// # use ryframe_middleware::rate_limit::RateLimiter;
/// # #[tokio::main]
/// # async fn main() {
/// // 创建内存限流器（自包含示例，无需外部依赖）
/// let limiter = RateLimiter::new_in_memory(100, 10);
/// assert!(limiter.try_acquire("test_key").await);
///
/// // 生成各类限流 key
/// assert_eq!(RateLimiter::user_key("42"), "user:42");
/// assert_eq!(RateLimiter::api_key("/api/login"), "api:/api/login");
/// assert_eq!(
///     RateLimiter::user_api_key("42", "/api/login"),
///     "user_api:42:/api/login"
/// );
/// # }
/// ```
pub async fn user_rate_limit_middleware(
    State(state): State<RateLimitState>,
    request: axum::extract::Request,
    next: Next,
) -> Result<Response, Response> {
    if !state.config.enabled || !state.config.enable_user_rate_limit {
        return Ok(next.run(request).await);
    }

    // 从 JWT Claims 提取 user_id（未认证用户走 IP 限流，不触发 user 限流）
    let user_key = request
        .extensions()
        .get::<ryframe_auth::jwt::Claims>()
        .map(|claims| RateLimiter::user_key(&claims.sub))
        .unwrap_or_default();

    // 未认证用户跳过用户级限流
    if user_key.is_empty() {
        return Ok(next.run(request).await);
    }

    let limit = state.config.user_capacity;
    let window = state.config.user_window_secs;

    let passed = state
        .limiter
        .acquire_configured(&user_key, window, limit, state.config.window_secs > 0)
        .await;

    if passed {
        Ok(next.run(request).await)
    } else {
        Err((
            StatusCode::TOO_MANY_REQUESTS,
            format!("用户请求过于频繁（{}秒内最多 {} 次）", window, limit),
        )
            .into_response())
    }
}

/// 接口级限流中间件
///
/// 根据 HTTP Method + Path 匹配配置中的 `api_limits`，对敏感接口单独限流。
///
/// 例如：登录接口 `POST /api/v1/auth/login` 每分钟最多 5 次。
pub async fn api_rate_limit_middleware(
    State(state): State<RateLimitState>,
    request: axum::extract::Request,
    next: Next,
) -> Result<Response, Response> {
    if !state.config.enabled || state.config.api_limits.is_empty() {
        return Ok(next.run(request).await);
    }

    let method = request.method().as_str();
    let path = request.uri().path().to_string();
    let window = state.config.api_window_secs;

    // 精确匹配 `METHOD /path`
    let exact_key = format!("{} {}", method, &path);
    // 通配匹配 `METHOD` 或 `/path`
    let method_key = method.to_string();

    let api_limit = state
        .config
        .api_limits
        .get(&exact_key)
        .or_else(|| state.config.api_limits.get(&path))
        .or_else(|| state.config.api_limits.get(&method_key));

    let Some(&limit) = api_limit else {
        return Ok(next.run(request).await);
    };

    let key = RateLimiter::api_key(&exact_key);
    let passed = state
        .limiter
        .acquire_configured(&key, window, limit, state.config.window_secs > 0)
        .await;

    if passed {
        Ok(next.run(request).await)
    } else {
        Err((
            StatusCode::TOO_MANY_REQUESTS,
            format!("接口请求过于频繁（{}秒内最多 {} 次）", window, limit),
        )
            .into_response())
    }
}
