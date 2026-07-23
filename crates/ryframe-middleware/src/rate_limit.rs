use std::{
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{MatchedPath, State},
    http::{HeaderValue, StatusCode, header::RETRY_AFTER},
    middleware::Next,
    response::{IntoResponse, Response},
};
use dashmap::DashMap;
use ryframe_common::utils::ip::{ClientIp, TrustedProxySet};
use ryframe_core::RedisClient;

use crate::metrics::{record_rate_limit_rejection, record_redis_degraded};

const RATE_LIMIT_KEY_PREFIX: &str = "ryframe:v0.5:rate-limit:";
const DEFAULT_WINDOW_SECS: u64 = 60;

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

    /// Create the development-only in-memory limiter.
    /// `refill_per_sec` is retained for source compatibility; fixed windows are
    /// used so configured API/user limits have identical semantics in both modes.
    pub fn new_in_memory(capacity: u32, refill_per_sec: u32) -> Self {
        let default_window_secs = if refill_per_sec == 0 {
            DEFAULT_WINDOW_SECS
        } else {
            (capacity.max(1) as u64)
                .div_ceil(refill_per_sec as u64)
                .max(1)
        };
        Self {
            mode: RateLimiterMode::InMemory {
                inner: Arc::new(RateLimiterInner {
                    buckets: DashMap::new(),
                    default_capacity: capacity.max(1),
                    default_window_secs,
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
                let script = r#"
                    local count = redis.call('INCR', KEYS[1])
                    if count == 1 then
                        redis.call('EXPIRE', KEYS[1], tonumber(ARGV[1]))
                    end
                    if count <= tonumber(ARGV[2]) then
                        return 1
                    end
                    return 0
                "#;
                let window_arg = window_secs.to_string();
                let limit_arg = limit.to_string();
                match client
                    .eval_script(
                        script,
                        &[redis_key.as_str()],
                        &[window_arg.as_str(), limit_arg.as_str()],
                    )
                    .await
                {
                    Ok(redis::Value::Int(value)) => Ok(RateLimitDecision {
                        allowed: value == 1,
                        retry_after_secs: window_secs,
                    }),
                    Ok(value) => Err(format!("unexpected Redis rate-limit result: {value:?}")),
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

    pub async fn sliding_window_acquire(&self, key: &str, window_secs: u64, limit: u32) -> bool {
        self.acquire(key, window_secs, limit)
            .await
            .is_ok_and(|decision| decision.allowed)
    }

    pub fn available_tokens(&self, key: &str) -> f64 {
        match &self.mode {
            RateLimiterMode::InMemory { inner } => inner
                .buckets
                .get(key)
                .map(|bucket| inner.default_capacity.saturating_sub(bucket.count) as f64)
                .unwrap_or(inner.default_capacity as f64),
            RateLimiterMode::Redis {
                default_capacity, ..
            } => *default_capacity as f64,
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

#[derive(Clone)]
pub struct RateLimitState {
    pub limiter: Arc<RateLimiter>,
    pub config: Arc<ryframe_config::RateLimitConfig>,
    pub trusted_proxies: TrustedProxySet,
}

impl RateLimitState {
    pub fn client_ip(&self, headers: &axum::http::HeaderMap, peer: IpAddr) -> IpAddr {
        self.trusted_proxies.client_ip(headers, peer)
    }
}

pub async fn rate_limit_middleware(
    State(state): State<RateLimitState>,
    request: axum::extract::Request,
    next: Next,
) -> Result<Response, Response> {
    if !state.config.enabled {
        return Ok(next.run(request).await);
    }

    let client_ip = request
        .extensions()
        .get::<ClientIp>()
        .map(|value| value.0)
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let window = if state.config.window_secs == 0 {
        DEFAULT_WINDOW_SECS
    } else {
        state.config.window_secs
    };
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
    if !state.config.enabled || !state.config.enable_user_rate_limit {
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
    if !state.config.enabled || state.config.api_limits.is_empty() {
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
    // Scope the bucket to the rule that matched. In particular, all concrete
    // IDs for a `{param}` route and all paths for a method-wide rule must share
    // one bucket; otherwise clients can evade the limit by varying the URL.
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
