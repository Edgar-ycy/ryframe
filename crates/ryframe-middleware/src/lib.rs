pub mod body_limit;
pub mod cors;
pub mod rate_limit;
pub mod request_id;
pub mod request_log;
pub mod timeout;
pub mod xss_filter;

pub use body_limit::body_limit_middleware;
pub use cors::cors_layer;
pub use rate_limit::{RateLimiter, rate_limit_middleware};
pub use request_id::request_id_middleware;
pub use request_log::request_log_layer;
pub use timeout::timeout_middleware;
pub use xss_filter::xss_filter;

use tower_http::compression::CompressionLayer;

/// 响应压缩层（gzip + brotli）
pub fn compression_layer() -> CompressionLayer {
    CompressionLayer::new()
}
