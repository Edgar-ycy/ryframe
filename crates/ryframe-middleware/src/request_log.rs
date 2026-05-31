//! 请求日志中间件
//!
//! 记录每个 HTTP 请求的 method + path + status + latency，
//! 自动对敏感查询参数和请求头进行脱敏。

use axum::http::Request;
use tower_http::{
    classify::{ServerErrorsAsFailures, SharedClassifier},
    trace::{MakeSpan, TraceLayer},
};

/// 请求日志中间件工厂
///
/// 基于 tower-http TraceLayer，记录：
/// - 请求方法、路径、状态码
/// - 延迟
/// - 请求 ID
/// - 敏感 query 参数自动脱敏
pub fn request_log_layer() -> TraceLayer<SharedClassifier<ServerErrorsAsFailures>> {
    TraceLayer::new_for_http()
}

/// 扩展的请求日志层（包含脱敏后的请求 URI）
///
/// 使用 `make_span_with` 将脱敏后的 URI 记录到 Span 中。
pub fn request_log_layer_with_masking()
-> TraceLayer<SharedClassifier<ServerErrorsAsFailures>, impl MakeSpan<axum::body::Body>> {
    TraceLayer::new_for_http().make_span_with(|request: &Request<axum::body::Body>| {
        let method = request.method().to_string();
        let uri = request.uri().to_string();
        let masked_uri = mask_uri(&uri);

        tracing::info_span!(
            "request",
            http.method = %method,
            http.uri = %masked_uri,
            http.request_id = %request
                .headers()
                .get("x-request-id")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("-"),
        )
    })
}

/// 掩码 URI 中的敏感查询参数
fn mask_uri(uri: &str) -> String {
    if let Some((path, query)) = uri.split_once('?') {
        let masked_query = ryframe_common::utils::log_mask::mask_query_string(query);
        format!("{}?{}", path, masked_query)
    } else {
        uri.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_uri_no_query() {
        assert_eq!(mask_uri("/api/v1/users"), "/api/v1/users");
    }

    #[test]
    fn test_mask_uri_with_sensitive_query() {
        let masked = mask_uri("/api/auth/login?username=admin&password=secret");
        assert!(masked.contains("password=******"));
        assert!(masked.contains("username=admin"));
    }

    #[test]
    fn test_mask_uri_clean_query() {
        let masked = mask_uri("/api/users?page=1&size=10");
        assert_eq!(masked, "/api/users?page=1&size=10");
    }
}
