//! 流式请求体安全大小限制。

use axum::{
    Json,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use ryframe_config::UploadLimitsConfig;
use ryframe_http::{API_PREFIX, ApiResponse};
use ryframe_kernel::ErrorCode;

pub const FILE_UPLOAD_LIMIT_BYTES: usize = 10 * 1024 * 1024;
pub const AVATAR_UPLOAD_LIMIT_BYTES: usize = 5 * 1024 * 1024;

/// 在将请求交给提取器前，最多缓冲路由特定的限制大小。
/// `to_bytes` 会在消费请求体时强制该限制，因此分块请求也无法绕过与 Content-Length
/// 相同的 413 边界。
pub async fn body_limit_middleware(
    State(config): State<UploadLimitsConfig>,
    request: Request,
    next: Next,
) -> Response {
    let limit = request_body_limit(&config, request.uri().path());
    let (parts, body) = request.into_parts();
    let bytes = match to_bytes(body, limit).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(%error, limit_bytes = limit, "request body exceeded its limit");
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(ApiResponse::<()>::fail(
                    StatusCode::PAYLOAD_TOO_LARGE.as_u16(),
                    "请求体过大",
                    ErrorCode::PayloadTooLarge.as_str(),
                )),
            )
                .into_response();
        }
    };

    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

pub fn request_body_limit(config: &UploadLimitsConfig, path: &str) -> usize {
    if is_avatar_upload(path) {
        config.avatar_max_bytes + config.multipart_envelope_bytes
    } else {
        config.file_max_bytes + config.multipart_envelope_bytes
    }
}

fn is_avatar_upload(path: &str) -> bool {
    matches!(
        path.strip_prefix(API_PREFIX),
        Some("/auth/profile/avatar" | "/common/upload/avatar")
    )
}
