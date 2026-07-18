//! Streaming-safe request body limits.

use axum::{
    body::{Body, to_bytes},
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use ryframe_config::UploadLimitsConfig;

pub const FILE_UPLOAD_LIMIT_BYTES: usize = 10 * 1024 * 1024;
pub const AVATAR_UPLOAD_LIMIT_BYTES: usize = 5 * 1024 * 1024;

/// Buffer at most the route-specific limit before handing the request to an
/// extractor. `to_bytes` enforces the limit while consuming the body, so a
/// chunked request cannot bypass the same 413 boundary used for Content-Length.
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
            let mut response = (
                StatusCode::PAYLOAD_TOO_LARGE,
                r#"{"code":413,"msg":"request body is too large"}"#,
            )
                .into_response();
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("application/json"),
            );
            return response;
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
        path,
        "/api/v1/auth/profile/avatar" | "/api/v1/common/upload/avatar"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avatar_routes_use_the_smaller_limit() {
        let config = UploadLimitsConfig::default();
        assert_eq!(
            request_body_limit(&config, "/api/v1/auth/profile/avatar"),
            AVATAR_UPLOAD_LIMIT_BYTES + config.multipart_envelope_bytes
        );
        assert_eq!(
            request_body_limit(&config, "/api/v1/common/upload/avatar"),
            AVATAR_UPLOAD_LIMIT_BYTES + config.multipart_envelope_bytes
        );
        assert_eq!(
            request_body_limit(&config, "/api/v1/common/upload"),
            FILE_UPLOAD_LIMIT_BYTES + config.multipart_envelope_bytes
        );
    }
}
