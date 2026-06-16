//! 幂等性中间件
//!
//! 通过 `Idempotency-Key` 请求头实现防重复提交：
//! - 首次请求：执行业务逻辑，将结果缓存（Redis 或内存）
//! - 重复请求（相同 key）：直接返回缓存结果，不执行重复操作
//!
//! Redis 模式支持分布式幂等，内存模式适用于单机场景。
//! 缓存结果带有 TTL，过期后自动清理。

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use dashmap::DashMap;
use ryframe_core::RedisClient;
use serde::{Deserialize, Serialize};

/// 幂等性中间件状态
#[derive(Clone)]
pub struct IdempotencyState {
    redis: Option<RedisClient>,
    /// 内存模式存储
    local: Arc<DashMap<String, CachedResponse>>,
    /// 缓存 TTL（秒）
    ttl_seconds: u64,
}

/// 缓存的响应
#[derive(Clone, Serialize, Deserialize)]
struct CachedResponse {
    status: u16,
    body: Vec<u8>,
    content_type: String,
    /// 过期时刻（仅内存模式使用）
    #[serde(skip, default = "Instant::now")]
    expiry: Instant,
}

impl IdempotencyState {
    /// 创建幂等性状态
    pub fn new(redis: Option<RedisClient>, ttl_seconds: u64) -> Self {
        Self {
            redis,
            local: Arc::new(DashMap::new()),
            ttl_seconds,
        }
    }

    /// 生成 Redis key
    fn make_key(key: &str) -> String {
        format!("idempotency:{}", key)
    }

    /// 获取缓存结果
    async fn get(&self, key: &str) -> Option<Response> {
        if let Some(ref redis) = self.redis {
            let redis_key = Self::make_key(key);
            match redis.get(&redis_key).await {
                Ok(Some(json)) => {
                    if let Ok(cached) = serde_json::from_str::<CachedResponse>(&json) {
                        Some(rebuild_response(&cached))
                    } else {
                        None
                    }
                }
                _ => None,
            }
        } else {
            // 内存模式：检查并惰性删除过期条目
            if let Some(entry) = self.local.get(key) {
                if entry.expiry > Instant::now() {
                    let cached = entry.clone();
                    Some(rebuild_response(&cached))
                } else {
                    drop(entry);
                    self.local.remove(key);
                    None
                }
            } else {
                None
            }
        }
    }

    /// 缓存响应结果
    async fn set(&self, key: &str, status: u16, body_bytes: Vec<u8>, content_type: &str) {
        let cached = CachedResponse {
            status,
            body: body_bytes,
            content_type: content_type.to_string(),
            expiry: Instant::now() + Duration::from_secs(self.ttl_seconds),
        };

        if let Some(ref redis) = self.redis {
            let redis_key = Self::make_key(key);
            if let Ok(json) = serde_json::to_string(&cached) {
                let _ = redis.set_ex(redis_key, json, self.ttl_seconds).await;
            }
        } else {
            self.local.insert(key.to_string(), cached);
        }
    }

    /// 启动后台 GC（仅内存模式需要）
    pub fn spawn_gc(&self) {
        let local = self.local.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let now = Instant::now();
                local.retain(|_, cached| cached.expiry > now);
            }
        });
    }
}

/// 重建 Response from CachedResponse
fn rebuild_response(cached: &CachedResponse) -> Response {
    let status =
        axum::http::StatusCode::from_u16(cached.status).unwrap_or(axum::http::StatusCode::OK);
    let mut response = Response::new(Body::from(cached.body.clone()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        cached
            .content_type
            .parse()
            .unwrap_or(axum::http::HeaderValue::from_static("application/json")),
    );
    response.headers_mut().insert(
        "X-Idempotency-Replay",
        axum::http::HeaderValue::from_static("true"),
    );
    response
}

/// 幂等性中间件
///
/// 从请求头提取 `Idempotency-Key`，若 key 不存在则跳过幂等检查。
/// 若 key 存在：
/// - 首次请求：执行 → 缓存结果 → 返回
/// - 重复请求：直接返回缓存结果（加 `X-Idempotency-Replay: true` 头）
///
/// 使用方式（路由级）：
/// ```
/// use ryframe_middleware::idempotency::IdempotencyState;
///
/// // 创建幂等性状态（内存模式，自包含示例）
/// let state = IdempotencyState::new(None, 300);
///
/// // 注册为路由中间件：
/// // .route_layer(middleware::from_fn_with_state(state, idempotency_middleware))
/// ```
pub async fn idempotency_middleware(
    State(state): State<IdempotencyState>,
    request: Request,
    next: Next,
) -> Response {
    // 仅处理有 Idempotency-Key 的请求
    let idempotency_key = match request
        .headers()
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
    {
        Some(key) if !key.is_empty() => key.to_string(),
        _ => return next.run(request).await,
    };

    // 检查是否已有缓存结果
    if let Some(cached_response) = state.get(&idempotency_key).await {
        return cached_response;
    }

    // 首次请求：执行业务逻辑
    let response = next.run(request).await;

    // 只缓存成功响应（2xx）
    if response.status().is_success() {
        // 提取 body 字节以缓存
        let (parts, body) = response.into_parts();
        let content_type = parts
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/json");

        match axum::body::to_bytes(body, 1024 * 1024).await {
            Ok(bytes) => {
                let body_vec = bytes.to_vec();
                state
                    .set(
                        &idempotency_key,
                        parts.status.as_u16(),
                        body_vec.clone(),
                        content_type,
                    )
                    .await;
                // 重建响应
                Response::from_parts(parts, Body::from(bytes))
            }
            Err(_) => {
                // 无法提取 body，返回 500
                Response::builder()
                    .status(500)
                    .body(Body::from("Internal Server Error"))
                    .unwrap_or_else(|_| Response::new(Body::from("Internal Server Error")))
            }
        }
    } else {
        response
    }
}
