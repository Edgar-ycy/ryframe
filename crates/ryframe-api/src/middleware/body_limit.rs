//! API 流式请求体安全大小限制。

use std::error::Error;

use crate::http::{API_PREFIX, app_error_response};
use axum::{
    body::{Body, to_bytes},
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use ryframe_kernel::AppError;

use crate::settings::UploadSettings;

pub const FILE_UPLOAD_LIMIT_BYTES: usize = 10 * 1024 * 1024;
pub const AVATAR_UPLOAD_LIMIT_BYTES: usize = 5 * 1024 * 1024;

/// 在将请求交给提取器前，最多缓冲路由特定的限制大小。
/// `to_bytes` 会在消费请求体时强制该限制，因此分块请求也无法绕过与 Content-Length
/// 相同的 413 边界。
pub async fn body_limit_middleware(
    State(config): State<UploadSettings>,
    request: Request,
    next: Next,
) -> Response {
    let limit = request_body_limit(&config, request.uri().path());
    let (parts, body) = request.into_parts();
    let bytes = match to_bytes(body, limit).await {
        Ok(bytes) => bytes,
        Err(error) => {
            let error = if error
                .source()
                .is_some_and(|source| source.is::<http_body_util::LengthLimitError>())
            {
                AppError::PayloadTooLarge(format!("请求体超过 {limit} 字节限制"))
            } else {
                AppError::Internal(format!("读取请求体失败: {error:?}"))
            };
            return app_error_response(error);
        }
    };

    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

pub fn request_body_limit(config: &UploadSettings, path: &str) -> usize {
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
