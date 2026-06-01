//! 请求重放防护中间件
//!
//! 通过 `X-Nonce` 和 `X-Timestamp` 请求头防止请求重放攻击：
//! - Timestamp 校验：请求时间戳必须在允许的时间窗口内（默认 ±5 分钟）
//! - Nonce 唯一性：每个 nonce 只能使用一次，使用后存入 Redis/内存直到过期
//!
//! Redis 模式支持分布式防重放，内存模式适用于单机场景。

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use dashmap::DashMap;
use ryframe_core::RedisClient;

/// 重放防护中间件状态
#[derive(Clone)]
pub struct ReplayProtectionState {
    redis: Option<RedisClient>,
    /// 内存模式：nonce → 过期时刻
    local: Arc<DashMap<String, Instant>>,
    /// 时间窗口（秒），默认 300（5 分钟）
    window_seconds: i64,
}

impl ReplayProtectionState {
    /// 创建重放防护状态
    ///
    /// `window_seconds` - 允许的时间偏差（秒），建议 300（5 分钟）
    pub fn new(redis: Option<RedisClient>, window_seconds: i64) -> Self {
        Self {
            redis,
            local: Arc::new(DashMap::new()),
            window_seconds,
        }
    }

    /// 检查 nonce 是否已被使用
    ///
    /// 如果 nonce 未使用，标记为已使用并返回 true（通过校验）。
    /// 如果 nonce 已使用，返回 false（拒绝请求）。
    async fn check_and_mark_nonce(&self, nonce: &str) -> bool {
        if let Some(ref redis) = self.redis {
            let key = Self::nonce_key(nonce);
            // SET NX EX：原子地"不存在则设置"，返回是否设置成功
            let ttl = self.window_seconds as u64 * 2; // nonce 存活时间为窗口的 2 倍
            match redis::cmd("SET")
                .arg(&key)
                .arg("1")
                .arg("NX")
                .arg("EX")
                .arg(ttl)
                .query_async(&mut redis.conn().clone())
                .await
            {
                Ok(redis::Value::Okay) => true, // 设置成功 = nonce 未被使用
                _ => false,                     // nonce 已存在
            }
        } else {
            // 内存模式
            let now = Instant::now();
            // entry API: 如果不存在则插入
            match self.local.entry(nonce.to_string()) {
                dashmap::mapref::entry::Entry::Vacant(entry) => {
                    entry.insert(now + Duration::from_secs(self.window_seconds as u64 * 2));
                    true
                }
                dashmap::mapref::entry::Entry::Occupied(mut entry) => {
                    if entry.get() < &now {
                        // 已过期，可以重新使用
                        entry.insert(now + Duration::from_secs(self.window_seconds as u64 * 2));
                        true
                    } else {
                        false
                    }
                }
            }
        }
    }

    fn nonce_key(nonce: &str) -> String {
        format!("replay_nonce:{}", nonce)
    }

    /// 启动后台 GC（仅内存模式需要）
    pub fn spawn_gc(&self) {
        let local = self.local.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let now = Instant::now();
                local.retain(|_, expiry| *expiry > now);
            }
        });
    }
}

/// 重放防护中间件
///
/// 从请求头提取 `X-Timestamp` 和 `X-Nonce`：
/// - 时间戳必须在 [now - window, now + window] 范围内
/// - Nonce 必须未被使用过
///
/// 如果任一校验失败，返回 400/409。
/// 如果请求未包含这两个头，则跳过校验（兼容无需防重放的客户端）。
///
/// 使用方式：
/// ```
/// use ryframe_middleware::replay_protection::ReplayProtectionState;
///
/// // 创建重放防护状态（内存模式，自包含示例，窗口 5 分钟）
/// let state = ReplayProtectionState::new(None, 300);
/// // 启动后台 GC 清理过期 nonce
/// state.spawn_gc();
///
/// // 注册为路由中间件：
/// // .route_layer(middleware::from_fn_with_state(replay_state, replay_protection_middleware))
/// ```
pub async fn replay_protection_middleware(
    State(state): State<ReplayProtectionState>,
    request: Request,
    next: Next,
) -> Result<Response, Response> {
    // 提取 X-Timestamp
    let timestamp_str = match request
        .headers()
        .get("X-Timestamp")
        .and_then(|v| v.to_str().ok())
    {
        Some(ts) if !ts.is_empty() => ts,
        // 未提供时间戳，跳过校验
        _ => return Ok(next.run(request).await),
    };

    // 提取 X-Nonce
    let nonce = match request
        .headers()
        .get("X-Nonce")
        .and_then(|v| v.to_str().ok())
    {
        Some(n) if !n.is_empty() => n,
        // 提供了时间戳但未提供 nonce，拒绝
        _ => {
            let body = axum::Json(serde_json::json!({
                "code": 400,
                "message": "缺少 X-Nonce 请求头",
            }));
            return Err((axum::http::StatusCode::BAD_REQUEST, body).into_response());
        }
    };

    // 1. 校验时间戳
    let request_ts: i64 = match timestamp_str.parse() {
        Ok(ts) => ts,
        Err(_) => {
            let body = axum::Json(serde_json::json!({
                "code": 400,
                "message": "X-Timestamp 格式无效，应为 Unix 时间戳（秒）",
            }));
            return Err((axum::http::StatusCode::BAD_REQUEST, body).into_response());
        }
    };

    let now = Utc::now().timestamp();
    let delta = (now - request_ts).abs();
    if delta > state.window_seconds {
        let body = axum::Json(serde_json::json!({
            "code": 400,
            "message": format!("请求时间戳偏差过大（{}秒），最大允许 {} 秒", delta, state.window_seconds),
        }));
        return Err((axum::http::StatusCode::BAD_REQUEST, body).into_response());
    }

    // 2. 校验 nonce 唯一性
    if !state.check_and_mark_nonce(nonce).await {
        let body = axum::Json(serde_json::json!({
            "code": 409,
            "message": "请求已被重放，X-Nonce 重复使用",
        }));
        return Err((axum::http::StatusCode::CONFLICT, body).into_response());
    }

    Ok(next.run(request).await)
}
