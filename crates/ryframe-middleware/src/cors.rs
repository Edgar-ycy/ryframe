use ryframe_common::{AppError, AppResult};
use ryframe_config::CorsConfig;
use tower_http::cors::{AllowOrigin, CorsLayer};

/// 创建 CORS 层
///
/// 通过配置文件 `[cors]` section 中的 `allow_origins` 配置允许的源。
/// 空白来源列表拒绝跨域请求；生产环境必须显式配置管理端 Origin。
///
/// 示例：
/// ```toml
/// [cors]
/// allow_origins = ["http://localhost:80", "http://localhost:3000"]
/// ```
pub fn cors_layer(config: &CorsConfig) -> AppResult<CorsLayer> {
    let allow_origin = if config.allow_origins.is_empty() {
        tracing::info!("CORS: allow_origins 为空，拒绝所有跨域来源");
        None
    } else {
        tracing::info!("CORS: 允许来源 {:?}", config.allow_origins);
        let origins = config
            .allow_origins
            .iter()
            .map(|origin| {
                origin.parse().map_err(|error| {
                    AppError::Config(format!("无效的 CORS 来源 {origin:?}: {error}"))
                })
            })
            .collect::<AppResult<Vec<_>>>()?;
        Some(AllowOrigin::list(origins))
    };

    let mut layer = CorsLayer::new()
        .allow_methods([
            http::Method::GET,
            http::Method::POST,
            http::Method::PUT,
            http::Method::PATCH,
            http::Method::DELETE,
            http::Method::OPTIONS,
        ])
        .allow_headers([
            http::header::AUTHORIZATION,
            http::header::CONTENT_TYPE,
            http::header::ACCEPT,
            http::header::ORIGIN,
            http::header::ACCESS_CONTROL_REQUEST_METHOD,
            http::header::ACCESS_CONTROL_REQUEST_HEADERS,
            http::HeaderName::from_static("x-tenant-id"),
            http::HeaderName::from_static("x-csrf-token"),
            http::HeaderName::from_static("idempotency-key"),
            http::HeaderName::from_static("x-request-id"),
        ])
        .expose_headers([
            http::header::CONTENT_DISPOSITION,
            http::header::RETRY_AFTER,
            http::HeaderName::from_static("x-request-id"),
            http::HeaderName::from_static("x-idempotency-replay"),
        ])
        .allow_credentials(true)
        .max_age(std::time::Duration::from_secs(3600));
    if let Some(allow_origin) = allow_origin {
        layer = layer.allow_origin(allow_origin);
    }
    Ok(layer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, http::Request, routing::get};
    use tower::ServiceExt;

    #[test]
    fn rejects_invalid_origin_without_panicking() {
        let config = CorsConfig {
            allow_origins: vec!["invalid\norigin".into()],
        };

        assert!(cors_layer(&config).is_err());
    }

    #[test]
    fn accepts_valid_origins() {
        let config = CorsConfig {
            allow_origins: vec!["http://localhost:5173".into()],
        };

        assert!(cors_layer(&config).is_ok());
    }

    #[tokio::test]
    async fn allowed_credentialed_origin_is_echoed_with_vary() {
        let config = CorsConfig {
            allow_origins: vec!["https://admin.example.com".into()],
        };
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(cors_layer(&config).unwrap());
        let response = app
            .oneshot(
                Request::builder()
                    .method(http::Method::OPTIONS)
                    .uri("/")
                    .header(http::header::ORIGIN, "https://admin.example.com")
                    .header(http::header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                    .header(
                        http::header::ACCESS_CONTROL_REQUEST_HEADERS,
                        "x-csrf-token,idempotency-key,x-request-id",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response
                .headers()
                .get(http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "https://admin.example.com"
        );
        assert_eq!(
            response
                .headers()
                .get(http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
                .unwrap(),
            "true"
        );
        let vary = response
            .headers()
            .get_all(http::header::VARY)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect::<Vec<_>>()
            .join(",");
        assert!(vary.to_ascii_lowercase().contains("origin"));
    }

    #[tokio::test]
    async fn empty_origin_list_emits_no_cross_origin_permission() {
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(cors_layer(&CorsConfig::default()).unwrap());
        let response = app
            .oneshot(
                Request::builder()
                    .method(http::Method::OPTIONS)
                    .uri("/")
                    .header(http::header::ORIGIN, "https://untrusted.example")
                    .header(http::header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(
            response
                .headers()
                .get(http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
    }
}
