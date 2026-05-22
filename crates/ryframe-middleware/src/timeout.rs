use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::time::Duration;

/// 请求超时中间件
///
/// 如果请求处理超过指定时间，返回 408 Request Timeout。
pub async fn timeout_middleware(request: Request, next: Next) -> Response {
    let timeout_duration = Duration::from_secs(30);

    match tokio::time::timeout(timeout_duration, next.run(request)).await {
        Ok(response) => response,
        Err(_) => {
            tracing::warn!("请求处理超时 ({}s)", timeout_duration.as_secs());
            (
                http::StatusCode::REQUEST_TIMEOUT,
                r#"{"code":408,"message":"请求处理超时，请稍后重试"}"#,
            )
                .into_response()
        }
    }
}
