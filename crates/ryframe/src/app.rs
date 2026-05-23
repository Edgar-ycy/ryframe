use axum::{routing::get, Router};
use ryframe_config::CorsConfig;
use std::sync::Arc;

/// 健康检查 Handler
async fn health_check() -> &'static str {
    r#"{"status":"ok"}"#
}

/// 构建 Axum Router
///
/// 注册中间件管道 + 所有业务模块的路由。
/// 中间件按从下到上顺序执行（最后注册的先执行）。
pub fn build_app(state: ryframe_api::AppState, limiter: Arc<ryframe_middleware::RateLimiter>, cors_config: &CorsConfig) -> Router {
    Router::new()
        .route("/", get(health_check))
        // 中间件层（从下到上执行）：
        // 1. 限流 (最外层，最先执行)
        .layer(
            axum::middleware::from_fn_with_state(
                limiter,
                ryframe_middleware::rate_limit_middleware,
            ),
        )
        // 2. 请求体大小限制 (10MB)
        .layer(axum::middleware::from_fn(
            ryframe_middleware::body_limit_middleware,
        ))
        // 3. 请求超时 (30秒)
        .layer(axum::middleware::from_fn(
            ryframe_middleware::timeout_middleware,
        ))
        // 3. XSS 过滤
        .layer(axum::middleware::from_fn(
            ryframe_middleware::xss_filter,
        ))
        // 4. 请求日志
        .layer(ryframe_middleware::request_log_layer())
        // 5. CORS
        .layer(ryframe_middleware::cors_layer(cors_config))
        // 6. 响应压缩 (最内层，最后执行)
        .layer(ryframe_middleware::compression_layer())
        // 7. Request ID
        .layer(axum::middleware::from_fn(
            ryframe_middleware::request_id_middleware,
        ))
        .nest("/api/v1", ryframe_api::api_router(state))
}