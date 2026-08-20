//! HTTP 请求限流状态与响应包装。

use std::{
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
};

use crate::ClientIp;
use axum::{
    extract::{MatchedPath, State},
    http::{HeaderValue, StatusCode, header::RETRY_AFTER},
    middleware::Next,
    response::{IntoResponse, Response},
};
use ryframe_adapters::{
    metrics::{record_rate_limit_rejection, record_redis_degraded},
    rate_limit::RateLimiter,
};

use crate::settings::RateLimitSettings;

#[derive(Clone)]
pub struct RateLimitState {
    pub limiter: Arc<RateLimiter>,
    pub config: Arc<RateLimitSettings>,
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
    // 将固定窗口限定到命中的规则，防止客户端通过变换具体 URL 绕过共享限额。
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

/// Agent API 使用覆盖身份、能力及并发维度的专用原子限流器。
fn is_agent_api_path(path: &str) -> bool {
    path == "/api/v1/agent/v1" || path.starts_with("/api/v1/agent/v1/")
}

fn rate_limited_response(scope: &str, message: &str, retry_after_secs: u64) -> Response {
    record_rate_limit_rejection(scope);
    let mut response = (StatusCode::TOO_MANY_REQUESTS, message.to_owned()).into_response();
    if let Ok(value) = HeaderValue::from_str(&retry_after_secs.max(1).to_string()) {
        response.headers_mut().insert(RETRY_AFTER, value);
    }
    response
}

fn rate_limit_unavailable(error: String) -> Response {
    record_redis_degraded("rate_limit");
    tracing::error!(error = %error, "限流后端不可用");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "限流服务暂不可用，请稍后重试",
    )
        .into_response()
}
