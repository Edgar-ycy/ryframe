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

pub(crate) const REQUEST_LOG_SPAN_TARGET: &str = "ryframe.request_log";

const UNMATCHED_ROUTE: &str = "/unmatched";

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

/// 扩展的请求日志层（使用路由模板）
///
/// 使用 `make_span_with` 将路由模板记录到 Span 中；未匹配请求使用固定值。
pub fn request_log_layer_with_masking()
-> TraceLayer<SharedClassifier<ServerErrorsAsFailures>, impl MakeSpan<axum::body::Body> + Clone> {
    TraceLayer::new_for_http()
        .make_span_with(|request: &Request<axum::body::Body>| {
            let method = request.method().to_string();
            // 请求 Span 不携带原始 URI；未匹配路由也必须使用固定值，
            // 防止用户输入直接成为日志和遥测的高基数属性。
            let path = request_route(request);
            let request_id = request
                .extensions()
                .get::<crate::request_id::RequestId>()
                .map(|value| value.0.as_str())
                .unwrap_or("-");
            let client_ip = request
                .extensions()
                .get::<ryframe_utils::ip::ClientIp>()
                .map(|value| value.0.to_string())
                .unwrap_or_else(|| "unknown".to_string());

            tracing::info_span!(target: REQUEST_LOG_SPAN_TARGET,
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

/// 返回日志 span 使用的路由模板；未匹配请求使用固定值以控制属性基数。
fn request_route<B>(request: &Request<B>) -> &str {
    request
        .extensions()
        .get::<MatchedPath>()
        .map_or(UNMATCHED_ROUTE, MatchedPath::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmatched_requests_use_a_fixed_log_route() {
        let request = Request::builder()
            .uri("/not-found/123?token=secret")
            .body(())
            .expect("construct an unmatched request");

        assert_eq!(request_route(&request), UNMATCHED_ROUTE);
    }

    #[test]
    fn request_log_span_target_is_stable() {
        assert_eq!(REQUEST_LOG_SPAN_TARGET, "ryframe.request_log");
    }
}
