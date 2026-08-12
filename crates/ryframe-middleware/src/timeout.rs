use std::time::Duration;

use axum::{
    Json,
    extract::{Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use ryframe_config::UploadLimitsConfig;
use ryframe_http::{API_PREFIX, ApiResponse};

pub const API_TIMEOUT_SECONDS: u64 = 30;
pub const UPLOAD_TIMEOUT_SECONDS: u64 = 120;

pub async fn timeout_middleware(
    State(config): State<UploadLimitsConfig>,
    request: Request,
    next: Next,
) -> Response {
    let timeout_seconds = request_timeout_seconds(&config, request.uri().path());
    match tokio::time::timeout(Duration::from_secs(timeout_seconds), next.run(request)).await {
        Ok(response) => response,
        Err(_) => {
            tracing::warn!(timeout_seconds, "HTTP request timed out");
            (
                http::StatusCode::REQUEST_TIMEOUT,
                Json(ApiResponse::<()>::fail(
                    http::StatusCode::REQUEST_TIMEOUT.as_u16(),
                    "请求处理超时",
                    "request_timeout",
                )),
            )
                .into_response()
        }
    }
}

pub fn request_timeout_seconds(config: &UploadLimitsConfig, path: &str) -> u64 {
    let api_path = path.strip_prefix(API_PREFIX);
    if api_path.is_some_and(|path| path.starts_with("/common/upload"))
        || matches!(
            api_path,
            Some(
                "/auth/profile/avatar" | "/system/user-imports" | "/system/config-transfers/upload"
            )
        )
    {
        config.upload_timeout_seconds
    } else {
        config.api_timeout_seconds
    }
}
