//! HTTP 传输中间件。

pub mod body_limit;
pub mod cache_control;
pub mod client_ip;
pub mod cors;
pub mod idempotency;
pub mod metrics;
pub mod rate_limit;
pub mod request_id;
pub mod request_log;
pub mod response_envelope;
pub mod security_headers;
pub mod telemetry;
pub mod timeout;

use tower_http::compression::CompressionLayer;

/// 创建 gzip 与 brotli 响应压缩层。
pub fn compression_layer() -> CompressionLayer {
    CompressionLayer::new()
}
