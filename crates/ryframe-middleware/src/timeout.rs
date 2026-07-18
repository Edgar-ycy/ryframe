use std::time::Duration;

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use ryframe_config::UploadLimitsConfig;

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
            let mut response = (
                http::StatusCode::REQUEST_TIMEOUT,
                r#"{"code":408,"msg":"request timed out"}"#,
            )
                .into_response();
            response.headers_mut().insert(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("application/json"),
            );
            response
        }
    }
}

pub fn request_timeout_seconds(config: &UploadLimitsConfig, path: &str) -> u64 {
    if path.starts_with("/api/v1/common/upload")
        || matches!(
            path,
            "/api/v1/auth/profile/avatar" | "/api/v1/system/users/import"
        )
    {
        config.upload_timeout_seconds
    } else {
        config.api_timeout_seconds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_upload_routes_receive_the_long_timeout() {
        let config = UploadLimitsConfig::default();
        assert_eq!(
            request_timeout_seconds(&config, "/api/v1/common/upload/image"),
            UPLOAD_TIMEOUT_SECONDS
        );
        assert_eq!(
            request_timeout_seconds(&config, "/api/v1/system/users/import"),
            UPLOAD_TIMEOUT_SECONDS
        );
        assert_eq!(
            request_timeout_seconds(&config, "/api/v1/system/users"),
            API_TIMEOUT_SECONDS
        );
    }
}
