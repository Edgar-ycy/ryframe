//! API 请求日志中间件
//!
//! 记录每个 HTTP 请求的 method + path + status + latency，
//! 自动对敏感查询参数和请求头进行脱敏。

use std::fmt;

use crate::http::ExpectedServiceUnavailableResponse;
use axum::{
    extract::MatchedPath,
    http::{Request, Response, StatusCode},
};
use tower_http::{
    LatencyUnit,
    classify::{
        ClassifiedResponse, ClassifyResponse, NeverClassifyEos, ServerErrorsFailureClass,
        SharedClassifier,
    },
    trace::{DefaultOnFailure, DefaultOnResponse, MakeSpan, TraceLayer},
};

const UNMATCHED_ROUTE: &str = "/unmatched";
pub const REQUEST_LOG_SPAN_TARGET: &str = "ryframe.request_log";

/// 请求日志的失败分类器。
///
/// 已由依赖状态边界确认的预期 503 会携带内部响应扩展。它们仍然是客户端可见的
/// 503，也会计入 HTTP 指标，但不再触发 tower-http 的 `on_failure` ERROR；其余
/// 5xx 和服务执行错误仍按失败处理。
#[derive(Clone, Debug, Default)]
pub struct RequestLogFailureClassifier;

impl ClassifyResponse for RequestLogFailureClassifier {
    type FailureClass = ServerErrorsFailureClass;
    type ClassifyEos = NeverClassifyEos<Self::FailureClass>;

    fn classify_response<B>(
        self,
        response: &Response<B>,
    ) -> ClassifiedResponse<Self::FailureClass, Self::ClassifyEos> {
        let status = response.status();
        let expected_service_unavailable = status == StatusCode::SERVICE_UNAVAILABLE
            && response
                .extensions()
                .get::<ExpectedServiceUnavailableResponse>()
                .is_some();
        if status.is_server_error() && !expected_service_unavailable {
            ClassifiedResponse::Ready(Err(ServerErrorsFailureClass::StatusCode(status)))
        } else {
            ClassifiedResponse::Ready(Ok(()))
        }
    }

    fn classify_error<E>(self, error: &E) -> Self::FailureClass
    where
        E: fmt::Display + 'static,
    {
        ServerErrorsFailureClass::Error(error.to_string())
    }
}

/// 请求日志中间件工厂
///
/// 基于 tower-http TraceLayer，记录：
/// - 请求方法、路径、状态码
/// - 延迟
/// - 请求 ID
/// - 敏感 query 参数自动脱敏
pub fn request_log_layer() -> TraceLayer<SharedClassifier<RequestLogFailureClassifier>> {
    TraceLayer::new(SharedClassifier::new(RequestLogFailureClassifier))
}

/// 扩展的请求日志层（使用路由模板）
///
/// 使用 `make_span_with` 将路由模板记录到 Span 中；未匹配请求使用固定值。
pub fn request_log_layer_with_masking() -> TraceLayer<
    SharedClassifier<RequestLogFailureClassifier>,
    impl MakeSpan<axum::body::Body> + Clone,
> {
    TraceLayer::new(SharedClassifier::new(RequestLogFailureClassifier))
        .make_span_with(|request: &Request<axum::body::Body>| {
            let method = request.method().to_string();
            // 请求 Span 不携带原始 URI；未匹配路由也必须使用固定值，
            // 防止用户输入直接成为日志和遥测的高基数属性。
            let path = request_route(request);
            let request_id = request
                .extensions()
                .get::<super::request_id::RequestId>()
                .map(|value| value.0.as_str())
                .unwrap_or("-");
            let client_ip = request
                .extensions()
                .get::<crate::ClientIp>()
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
