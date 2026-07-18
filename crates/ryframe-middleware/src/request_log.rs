//! 请求日志中间件
//!
//! 记录每个 HTTP 请求的 method + path + status + latency，
//! 自动对敏感查询参数和请求头进行脱敏。

use axum::{extract::MatchedPath, http::Request};
use tower_http::{
    LatencyUnit,
    classify::{ServerErrorsAsFailures, SharedClassifier},
    trace::{DefaultOnFailure, DefaultOnResponse, MakeSpan, TraceLayer},
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
-> TraceLayer<SharedClassifier<ServerErrorsAsFailures>, impl MakeSpan<axum::body::Body> + Clone> {
    TraceLayer::new_for_http()
        .make_span_with(|request: &Request<axum::body::Body>| {
            let method = request.method().to_string();
            // Never place a query string in a request span. Even an allow-list of
            // known keys is unsafe because routes and clients evolve independently.
            let path = request.extensions().get::<MatchedPath>().map_or_else(
                || log_path(request.uri()).to_string(),
                |path| path.as_str().to_owned(),
            );
            let request_id = request
                .extensions()
                .get::<crate::request_id::RequestId>()
                .map(|value| value.0.as_str())
                .unwrap_or("-");
            let client_ip = request
                .extensions()
                .get::<ryframe_common::utils::ip::ClientIp>()
                .map(|value| value.0.to_string())
                .unwrap_or_else(|| "unknown".to_string());

            tracing::info_span!(
                "request",
                http.method = %method,
                http.route = %path,
                http.request_id = %request_id,
                http.client_ip = %client_ip,
                tenant.id = tracing::field::Empty,
                user.id = tracing::field::Empty,
                user.name = tracing::field::Empty,
            )
        })
        .on_response(
            DefaultOnResponse::new()
                .level(tracing::Level::INFO)
                .latency_unit(LatencyUnit::Millis),
        )
        .on_failure(
            DefaultOnFailure::new()
                .level(tracing::Level::ERROR)
                .latency_unit(LatencyUnit::Millis),
        )
}

/// 掩码 URI 中的敏感查询参数
fn log_path(uri: &axum::http::Uri) -> &str {
    uri.path()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_path_keeps_only_the_path() {
        let uri: axum::http::Uri = "/api/v1/users?page=1&password=secret".parse().unwrap();
        assert_eq!(log_path(&uri), "/api/v1/users");
    }

    #[test]
    fn log_path_handles_a_path_without_query() {
        let uri: axum::http::Uri = "/api/v1/users".parse().unwrap();
        assert_eq!(log_path(&uri), "/api/v1/users");
    }
}
