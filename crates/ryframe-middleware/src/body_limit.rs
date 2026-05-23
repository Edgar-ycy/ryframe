//! 请求体大小限制中间件
//!
//! 防止超大请求体消耗服务器资源。默认限制 10 MB。

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// 默认请求体大小限制：10 MB
const DEFAULT_MAX_BODY_SIZE: usize = 10 * 1024 * 1024;

/// 请求体大小限制中间件
///
/// 检查 Content-Length 头，如果超过限制则拒绝请求。
/// 对于没有 Content-Length 的请求（如 chunked），由 Axum 内置的 body limit 处理。
pub async fn body_limit_middleware(request: Request, next: Next) -> Response {
    let max_size = DEFAULT_MAX_BODY_SIZE;

    // 检查 Content-Length
    let too_large = request
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok())
        .is_some_and(|len| len > max_size);

    if too_large {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                r#"{{"code":413,"msg":"请求体超过大小限制（最大 {} MB）"}}"#,
                max_size / 1024 / 1024
            ),
        )
            .into_response();
    }

    next.run(request).await
}
