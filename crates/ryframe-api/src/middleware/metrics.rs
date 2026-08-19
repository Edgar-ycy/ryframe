//! Axum 请求指标采集边界。

use axum::{
    extract::{MatchedPath, Request},
    middleware::Next,
    response::Response,
};
use ryframe_adapters::metrics::HttpRequestObservation;

const UNMATCHED_ROUTE: &str = "/unmatched";

pub async fn metrics_middleware(request: Request, next: Next) -> Response {
    let method = request.method().to_string();
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map_or(UNMATCHED_ROUTE, MatchedPath::as_str)
        .to_owned();
    let observation = HttpRequestObservation::start(method, path);
    let response = next.run(request).await;
    observation.finish(response.status().as_u16());
    response
}
