pub mod body_limit;
pub mod cache_control;
pub mod client_ip;
pub mod cors;
pub mod idempotency;
pub mod metrics;
pub mod rate_limit;
pub mod replay_protection;
pub mod request_id;
pub mod request_log;
pub mod security_headers;
pub mod telemetry;
pub mod timeout;
pub mod websocket;
pub mod xss_filter;

pub use body_limit::body_limit_middleware;
pub use cache_control::{CacheControlConfig, cache_control_middleware};
pub use client_ip::trusted_client_ip_middleware;
pub use cors::cors_layer;
pub use idempotency::{IdempotencyState, idempotency_middleware};
pub use rate_limit::{
    RateLimitState, RateLimiter, api_rate_limit_middleware, rate_limit_middleware,
    user_rate_limit_middleware,
};
pub use replay_protection::{ReplayProtectionState, replay_protection_middleware};
pub use request_id::request_id_middleware;
pub use request_log::{request_log_layer, request_log_layer_with_masking};
pub use security_headers::{SecurityHeadersConfig, security_headers_middleware};
pub use timeout::timeout_middleware;
use tower_http::compression::CompressionLayer;
pub use xss_filter::xss_filter;

/// 响应压缩层（gzip + brotli）
pub fn compression_layer() -> CompressionLayer {
    CompressionLayer::new()
}
