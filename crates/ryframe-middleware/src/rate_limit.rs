use std::{
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{MatchedPath, State},
    http::{HeaderMap, HeaderValue, StatusCode, header::RETRY_AFTER},
    middleware::Next,
    response::{IntoResponse, Response},
};
use dashmap::DashMap;
use redis::AsyncCommands;
use ryframe_adapters::RedisClient;
use ryframe_utils::ip::{ClientIp, TrustedProxySet};

use crate::metrics::{record_rate_limit_rejection, record_redis_degraded};

const RATE_LIMIT_KEY_PREFIX: &str = "ryframe:v0.5:rate-limit:";
#[derive(Debug)]
struct WindowBucket {
    count: u32,
    reset_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitDecision {
    pub allowed: bool,
    pub retry_after_secs: u64,
}

/// 固定窗口限流器的只读快照。
///
/// 快照不会创建桶或递增计数，仅用于展示当前窗口；`remaining_secs = 0` 表示当前没有活跃窗口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitSnapshot {
    pub key: String,
    pub current: u64,
    pub limit: u32,
    pub remaining_secs: u64,
}

#[derive(Clone)]
pub struct RateLimiter {
    mode: RateLimiterMode,
}

#[derive(Clone)]
enum RateLimiterMode {
    Redis {
        client: Box<RedisClient>,
        default_capacity: u32,
        default_window_secs: u64,
    },
    InMemory {
        inner: Arc<RateLimiterInner>,
    },
}

struct RateLimiterInner {
    buckets: DashMap<String, WindowBucket>,
    default_capacity: u32,
    default_window_secs: u64,
}

impl RateLimiter {
    pub fn new_redis(client: RedisClient, capacity: u32, window_secs: u64) -> Self {
        Self {
            mode: RateLimiterMode::Redis {
                client: Box::new(client),
                default_capacity: capacity.max(1),
                default_window_secs: window_secs.max(1),
            },
        }
    }

    /// 创建仅用于开发与隔离环境的内存固定窗口限流器。
    pub fn new_in_memory(capacity: u32, window_secs: u64) -> Self {
        Self {
            mode: RateLimiterMode::InMemory {
                inner: Arc::new(RateLimiterInner {
                    buckets: DashMap::new(),
                    default_capacity: capacity.max(1),
                    default_window_secs: window_secs.max(1),
                }),
            },
        }
    }

    pub async fn acquire(
        &self,
        key: &str,
        window_secs: u64,
        limit: u32,
    ) -> Result<RateLimitDecision, String> {
        let window_secs = window_secs.max(1);
        let limit = limit.max(1);

        match &self.mode {
            RateLimiterMode::Redis { client, .. } => {
                let redis_key = format!("{RATE_LIMIT_KEY_PREFIX}{key}");
                let watched = [redis_key.clone()];
                match client
                    .transaction(&watched, move |mut connection, mut transaction| {
                        let redis_key = redis_key.clone();
                        async move {
                            let current: Option<u64> = connection.get(&redis_key).await?;
                            let ttl: i64 = connection.ttl(&redis_key).await?;
                            let count = current.unwrap_or(0).saturating_add(1);
                            transaction.incr(&redis_key, 1_u8).ignore();
                            if current.is_none() || ttl < 0 {
                                transaction
                                    .expire(&redis_key, redis_ttl_secs(window_secs))
                                    .ignore();
                            }
                            let committed: Option<()> =
                                transaction.query_async(&mut connection).await?;
                            Ok(committed.map(|()| count <= u64::from(limit)))
                        }
                    })
                    .await
                {
                    Ok(allowed) => Ok(RateLimitDecision {
                        allowed,
                        retry_after_secs: window_secs,
                    }),
                    Err(error) => Err(format!("Redis rate-limit operation failed: {error}")),
                }
            }
            RateLimiterMode::InMemory { inner } => {
                let now = Instant::now();
                let reset_at = now + Duration::from_secs(window_secs);
                let mut bucket = inner
                    .buckets
                    .entry(key.to_string())
                    .or_insert(WindowBucket { count: 0, reset_at });
                if bucket.reset_at <= now {
                    bucket.count = 0;
                    bucket.reset_at = reset_at;
                }
                bucket.count = bucket.count.saturating_add(1);
                Ok(RateLimitDecision {
                    allowed: bucket.count <= limit,
                    retry_after_secs: bucket
                        .reset_at
                        .saturating_duration_since(now)
                        .as_secs()
                        .max(1),
                })
            }
        }
    }

    pub async fn try_acquire(&self, key: &str) -> bool {
        let (capacity, window_secs) = self.default_rule();
        self.acquire(key, window_secs, capacity)
            .await
            .is_ok_and(|decision| decision.allowed)
    }

    /// 批量读取固定窗口状态，不改变任何限流计数。
    ///
    /// Redis 模式使用单次原子管道读取全部计数和 TTL；内存模式直接读取已有桶。
    pub async fn snapshot_many(
        &self,
        keys: &[String],
        limit: u32,
    ) -> Result<Vec<RateLimitSnapshot>, String> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        match &self.mode {
            RateLimiterMode::Redis { client, .. } => {
                let redis_keys = keys
                    .iter()
                    .map(|key| format!("{RATE_LIMIT_KEY_PREFIX}{key}"))
                    .collect::<Vec<_>>();
                let watched = redis_keys.clone();
                let values = client
                    .transaction(&watched, move |mut connection, mut transaction| {
                        let redis_keys = redis_keys.clone();
                        async move {
                            for redis_key in &redis_keys {
                                transaction.get(redis_key).ttl(redis_key);
                            }
                            transaction.query_async(&mut connection).await
                        }
                    })
                    .await
                    .map_err(|error| format!("Redis rate-limit snapshot failed: {error}"))?;
                parse_redis_snapshots(keys, limit, values)
            }
            RateLimiterMode::InMemory { inner } => {
                let now = Instant::now();
                Ok(keys
                    .iter()
                    .map(|key| {
                        let (current, remaining_secs) = inner
                            .buckets
                            .get(key)
                            .filter(|bucket| bucket.reset_at > now)
                            .map(|bucket| {
                                (
                                    u64::from(bucket.count),
                                    bucket
                                        .reset_at
                                        .saturating_duration_since(now)
                                        .as_secs()
                                        .max(1),
                                )
                            })
                            .unwrap_or((0, 0));
                        RateLimitSnapshot {
                            key: key.clone(),
                            current,
                            limit,
                            remaining_secs,
                        }
                    })
                    .collect())
            }
        }
    }

    pub fn spawn_gc(self: &Arc<Self>) {
        if let RateLimiterMode::InMemory { inner } = &self.mode {
            let inner = inner.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(60));
                loop {
                    interval.tick().await;
                    let now = Instant::now();
                    inner.buckets.retain(|_, bucket| bucket.reset_at > now);
                }
            });
        }
    }

    fn default_rule(&self) -> (u32, u64) {
        match &self.mode {
            RateLimiterMode::Redis {
                default_capacity,
                default_window_secs,
                ..
            } => (*default_capacity, *default_window_secs),
            RateLimiterMode::InMemory { inner } => {
                (inner.default_capacity, inner.default_window_secs)
            }
        }
    }

    pub fn user_key(user_id: &str) -> String {
        format!("user:{user_id}")
    }

    pub fn tenant_user_key(tenant_id: &str, user_id: &str) -> String {
        format!("tenant_user:{tenant_id}:{user_id}")
    }

    pub fn tenant_key(tenant_id: &str) -> String {
        format!("tenant:{tenant_id}")
    }

    pub fn api_key(path: &str) -> String {
        format!("api:{path}")
    }

    pub fn api_client_key(path: &str, client_ip: IpAddr) -> String {
        format!("api:{path}:ip:{client_ip}")
    }

    pub fn user_api_key(user_id: &str, path: &str) -> String {
        format!("user_api:{user_id}:{path}")
    }
}

fn parse_redis_snapshots(
    keys: &[String],
    limit: u32,
    values: Vec<redis::Value>,
) -> Result<Vec<RateLimitSnapshot>, String> {
    if values.len() != keys.len().saturating_mul(2) {
        return Err("Redis rate-limit snapshot returned an invalid item count".into());
    }
    keys.iter()
        .enumerate()
        .map(|(index, key)| {
            let current = redis_snapshot_integer(&values[index * 2])?;
            let remaining_secs = redis_snapshot_integer(&values[index * 2 + 1])?;
            Ok(RateLimitSnapshot {
                key: key.clone(),
                current,
                limit,
                remaining_secs,
            })
        })
        .collect()
}

fn redis_snapshot_integer(value: &redis::Value) -> Result<u64, String> {
    match value {
        // 不存在的窗口：`GET` 返回 nil、`TTL` 返回负整数，统一按“无活跃窗口”读取。
        redis::Value::Nil => Ok(0),
        redis::Value::Int(value) if *value < 0 => Ok(0),
        // `INCR` 写出的整数在部分驱动路径下以整数回复返回，`GET` 路径则
        // 以 bulk string 返回；两种类型都应解析，避免读取方与写入方协议不一致。
        redis::Value::Int(value) => u64::try_from(*value)
            .map_err(|_| "Redis rate-limit snapshot returned a negative value".to_owned()),
        redis::Value::BulkString(bytes) => std::str::from_utf8(bytes)
            .ok()
            .and_then(|text| text.parse().ok())
            .ok_or_else(|| format!("unexpected Redis rate-limit snapshot item: {value:?}")),
        _ => Err(format!(
            "unexpected Redis rate-limit snapshot item: {value:?}"
        )),
    }
}

fn redis_ttl_secs(ttl_secs: u64) -> i64 {
    ttl_secs.min(i64::MAX as u64) as i64
}

#[derive(Clone)]
pub struct RateLimitState {
    pub limiter: Arc<RateLimiter>,
    pub config: Arc<ryframe_config::RateLimitConfig>,
    pub trusted_proxies: TrustedProxySet,
}

impl RateLimitState {
    pub fn client_ip(&self, headers: &HeaderMap, peer: IpAddr) -> IpAddr {
        self.trusted_proxies.client_ip(headers, peer)
    }
}

pub async fn rate_limit_middleware(
    State(state): State<RateLimitState>,
    request: axum::extract::Request,
    next: Next,
) -> Result<Response, Response> {
    if !state.config.enabled || is_agent_api_path(request.uri().path()) {
        return Ok(next.run(request).await);
    }

    let client_ip = request
        .extensions()
        .get::<ClientIp>()
        .map(|value| value.0)
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let window = state.config.window_secs;
    match state
        .limiter
        .acquire(
            &format!("global:ip:{client_ip}"),
            window,
            state.config.capacity,
        )
        .await
    {
        Ok(decision) if decision.allowed => Ok(next.run(request).await),
        Ok(decision) => Err(rate_limited_response(
            "global_ip",
            "请求过于频繁，请稍后再试",
            decision.retry_after_secs,
        )),
        Err(error) => Err(rate_limit_unavailable(error)),
    }
}

pub async fn user_rate_limit_middleware(
    State(state): State<RateLimitState>,
    request: axum::extract::Request,
    next: Next,
) -> Result<Response, Response> {
    if !state.config.enabled
        || !state.config.enable_user_rate_limit
        || is_agent_api_path(request.uri().path())
    {
        return Ok(next.run(request).await);
    }

    let Some(claims) = request.extensions().get::<ryframe_auth::jwt::Claims>() else {
        return Ok(next.run(request).await);
    };
    let key = RateLimiter::tenant_user_key(&claims.tenant_id, &claims.sub);
    match state
        .limiter
        .acquire(
            &key,
            state.config.user_window_secs,
            state.config.user_capacity,
        )
        .await
    {
        Ok(decision) if decision.allowed => Ok(next.run(request).await),
        Ok(decision) => Err(rate_limited_response(
            "tenant_user",
            "用户请求过于频繁，请稍后再试",
            decision.retry_after_secs,
        )),
        Err(error) => Err(rate_limit_unavailable(error)),
    }
}

pub async fn api_rate_limit_middleware(
    State(state): State<RateLimitState>,
    request: axum::extract::Request,
    next: Next,
) -> Result<Response, Response> {
    if !state.config.enabled
        || state.config.api_limits.is_empty()
        || is_agent_api_path(request.uri().path())
    {
        return Ok(next.run(request).await);
    }

    let method = request.method().as_str();
    let concrete_path = request.uri().path();
    let route_path = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or(concrete_path);
    let method_concrete_rule = format!("{method} {concrete_path}");
    let method_route_rule = format!("{method} {route_path}");
    let configured_rule = state
        .config
        .api_limits
        .get(&method_concrete_rule)
        .map(|limit| (method_concrete_rule.as_str(), *limit))
        .or_else(|| {
            state
                .config
                .api_limits
                .get(&method_route_rule)
                .map(|limit| (method_route_rule.as_str(), *limit))
        })
        .or_else(|| {
            state
                .config
                .api_limits
                .get(concrete_path)
                .map(|limit| (concrete_path, *limit))
        })
        .or_else(|| {
            state
                .config
                .api_limits
                .get(route_path)
                .map(|limit| (route_path, *limit))
        })
        .or_else(|| {
            state
                .config
                .api_limits
                .get(method)
                .map(|limit| (method, *limit))
        });
    let Some((rule_scope, limit)) = configured_rule else {
        return Ok(next.run(request).await);
    };

    let client_ip = request
        .extensions()
        .get::<ClientIp>()
        .map(|value| value.0)
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    // 将固定窗口限定到命中的规则。尤其是，`{param}` 路由的所有具体 ID 以及方法级规则的所有路径
    // 必须共享同一个窗口；否则客户端可通过变换 URL 绕过限额。
    let key = RateLimiter::api_client_key(rule_scope, client_ip);
    match state
        .limiter
        .acquire(&key, state.config.api_window_secs, limit)
        .await
    {
        Ok(decision) if decision.allowed => Ok(next.run(request).await),
        Ok(decision) => Err(rate_limited_response(
            "api_ip",
            "接口请求过于频繁，请稍后再试",
            decision.retry_after_secs,
        )),
        Err(error) => Err(rate_limit_unavailable(error)),
    }
}

/// Agent API 使用能够覆盖身份、能力及并发维度的专用原子限流器；通用限流不得提前返回未审计的 429。
fn is_agent_api_path(path: &str) -> bool {
    path == "/api/v1/agent/v1" || path.starts_with("/api/v1/agent/v1/")
}

fn rate_limited_response(scope: &str, message: &str, retry_after_secs: u64) -> Response {
    record_rate_limit_rejection(scope);
    let mut response = (StatusCode::TOO_MANY_REQUESTS, message.to_string()).into_response();
    if let Ok(value) = HeaderValue::from_str(&retry_after_secs.max(1).to_string()) {
        response.headers_mut().insert(RETRY_AFTER, value);
    }
    response
}

fn rate_limit_unavailable(error: String) -> Response {
    record_redis_degraded("rate_limit");
    tracing::error!(error = %error, "rate-limit backend unavailable");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "限流服务暂不可用，请稍后重试",
    )
        .into_response()
}
