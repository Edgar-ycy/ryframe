use std::sync::Arc;

use axum::{Router, middleware::from_fn, routing::get};
use ryframe_config::CorsConfig;
use ryframe_middleware::rate_limit::RateLimitState;

/// 健康检查 Handler
async fn health_check() -> &'static str {
    r#"{"status":"ok"}"#
}

/// 构建 Axum Router
///
/// 注册中间件管道 + 所有业务模块的路由。
/// 中间件按从下到上顺序执行（最后注册的先执行）。
pub fn build_app(
    state: ryframe_api::AppState,
    limiter: Arc<ryframe_middleware::RateLimiter>,
    rate_limit_state: RateLimitState,
    cors_config: &CorsConfig,
) -> Router {
    Router::new()
        .route("/", get(health_check))
        .route("/health", get(health_check))
        // 中间件层（从下到上执行）：
        // 1. 限流 (最外层，最先执行)
        .layer(axum::middleware::from_fn_with_state(
            limiter,
            ryframe_middleware::rate_limit_middleware,
        ))
        // 2. 接口级限流（敏感接口如 POST /api/v1/auth/login 单独限制）
        .layer(axum::middleware::from_fn_with_state(
            rate_limit_state,
            ryframe_middleware::api_rate_limit_middleware,
        ))
        // 3. 请求体大小限制 (10MB)
        .layer(axum::middleware::from_fn(
            ryframe_middleware::body_limit_middleware,
        ))
        // 4. 请求超时 (30秒)
        .layer(axum::middleware::from_fn(
            ryframe_middleware::timeout_middleware,
        ))
        // 5. XSS 过滤
        .layer(axum::middleware::from_fn(ryframe_middleware::xss_filter))
        // 6. 请求日志
        .layer(ryframe_middleware::request_log_layer())
        // 7. CORS
        .layer(ryframe_middleware::cors_layer(cors_config))
        // 8. 响应压缩 (最内层，最后执行)
        .layer(ryframe_middleware::compression_layer())
        // 9. Request ID
        .layer(axum::middleware::from_fn(
            ryframe_middleware::request_id_middleware,
        ))
        // 10. 链路追踪 Span（在 request_id 之后，读取 x-request-id）
        .layer(from_fn(ryframe_middleware::telemetry::telemetry_middleware))
        // 11. HTTP Metrics（最内层，最先执行，捕获完整请求）
        .layer(from_fn(ryframe_middleware::metrics::metrics_middleware))
        .nest("/api/v1", ryframe_api::api_router(state))
}
