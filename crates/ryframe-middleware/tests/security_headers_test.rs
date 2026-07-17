/// security_headers 中间件测试
/// 从 crates/ryframe-middleware/src/security_headers.rs 内联测试迁移
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    middleware,
    routing::get,
};
use ryframe_middleware::security_headers::{SecurityHeadersConfig, security_headers_middleware};
use tower::util::ServiceExt;

/// 创建测试用的 Router
fn test_router(config: SecurityHeadersConfig) -> Router {
    async fn handler() -> &'static str {
        "ok"
    }

    Router::new()
        .route("/", get(handler))
        .layer(middleware::from_fn_with_state(
            config,
            security_headers_middleware,
        ))
}

#[tokio::test]
async fn test_default_security_headers() {
    let app = test_router(SecurityHeadersConfig::default());
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(
        response.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );
    assert!(response.headers().get("x-frame-options").is_some());
    let csp = response
        .headers()
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(csp.contains("img-src 'self' data: blob: https:"));
    assert!(
        response
            .headers()
            .get("strict-transport-security")
            .is_some()
    );
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_development_config_no_hsts() {
    let app = test_router(SecurityHeadersConfig::development());
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert!(
        response
            .headers()
            .get("strict-transport-security")
            .is_none()
    );
}

#[tokio::test]
async fn test_strict_config() {
    let app = test_router(SecurityHeadersConfig::strict());
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    let csp = response
        .headers()
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(csp.contains("frame-ancestors 'none'"));
    assert!(csp.contains("img-src 'self' data: blob: https:"));
}

#[tokio::test]
async fn test_custom_headers() {
    let mut config = SecurityHeadersConfig::default();
    config
        .custom_headers
        .insert("X-Custom-Security".to_string(), "test-value".to_string());

    let app = test_router(config);
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(
        response.headers().get("x-custom-security").unwrap(),
        "test-value"
    );
}

#[tokio::test]
async fn test_custom_x_frame_options() {
    let config = SecurityHeadersConfig {
        x_frame_options: Some("SAMEORIGIN".to_string()),
        ..Default::default()
    };

    let app = test_router(config);
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(
        response.headers().get("x-frame-options").unwrap(),
        "SAMEORIGIN"
    );
}

#[tokio::test]
async fn test_all_required_headers_present() {
    let app = test_router(SecurityHeadersConfig::default());
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    let headers = response.headers();
    assert!(headers.contains_key("x-content-type-options"));
    assert!(headers.contains_key("x-xss-protection"));
    assert!(headers.contains_key("x-frame-options"));
    assert!(headers.contains_key("content-security-policy"));
    assert!(headers.contains_key("strict-transport-security"));
    assert!(headers.contains_key("referrer-policy"));
    assert!(headers.contains_key("permissions-policy"));
    assert!(headers.contains_key("cross-origin-opener-policy"));
    assert!(headers.contains_key("cross-origin-resource-policy"));
    assert!(headers.contains_key("x-dns-prefetch-control"));
}
