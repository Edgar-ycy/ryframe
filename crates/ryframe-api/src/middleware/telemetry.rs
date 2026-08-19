//! HTTP 请求链路传播与 span 生命周期。

use axum::{
    extract::{MatchedPath, Request},
    http::HeaderMap,
    middleware::Next,
    response::Response,
};
use opentelemetry::{global, propagation::Extractor};
use tracing::{Instrument, error, info, warn};
use tracing_opentelemetry::OpenTelemetrySpanExt;

struct HeaderExtractor<'a>(&'a HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|key| key.as_str()).collect()
    }
}

/// 为 HTTP 请求创建 span，并记录低基数路由、状态与耗时。
pub async fn telemetry_middleware(request: Request, next: Next) -> Response {
    let method = request.method().to_string();
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned())
        .unwrap_or_else(|| "/unmatched".to_owned());
    let parent_context = global::get_text_map_propagator(|propagator| {
        propagator.extract(&HeaderExtractor(request.headers()))
    });

    let span = tracing::info_span!(
        "HTTP",
        http.method = %method,
        http.route = %path,
        http.status_code = tracing::field::Empty,
    );
    let _ = span.set_parent(parent_context);

    let started = std::time::Instant::now();
    let response = next.run(request).instrument(span.clone()).await;
    let elapsed = started.elapsed();
    let status = response.status().as_u16();
    span.record("http.status_code", status.to_string());

    match classify_http_response(status) {
        HttpResponseClass::ClientError => info!(
            http.status_code = status,
            http.duration_ms = elapsed.as_millis(),
            http.route = %path,
            "HTTP 客户端错误响应"
        ),
        HttpResponseClass::ServerError => error!(
            http.status_code = status,
            http.duration_ms = elapsed.as_millis(),
            http.route = %path,
            "HTTP 服务端错误响应"
        ),
        HttpResponseClass::Success => {}
    }

    if elapsed.as_millis() > 1000 {
        warn!(
            http.duration_ms = elapsed.as_millis(),
            http.route = %path,
            "慢请求"
        );
    }

    response
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HttpResponseClass {
    Success,
    ClientError,
    ServerError,
}

const fn classify_http_response(status: u16) -> HttpResponseClass {
    match status {
        500.. => HttpResponseClass::ServerError,
        400..=499 => HttpResponseClass::ClientError,
        _ => HttpResponseClass::Success,
    }
}

#[cfg(test)]
mod tests {
    use super::{HttpResponseClass, classify_http_response};

    #[test]
    fn response_classes_follow_http_status_ranges() {
        assert_eq!(classify_http_response(204), HttpResponseClass::Success);
        assert_eq!(classify_http_response(404), HttpResponseClass::ClientError);
        assert_eq!(classify_http_response(503), HttpResponseClass::ServerError);
    }
}
