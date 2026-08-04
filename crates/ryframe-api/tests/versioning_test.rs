use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    middleware::{from_fn, from_fn_with_state},
    routing::get,
};
use ryframe_api::router::api_version;
use ryframe_api::versioning::{ApiVersion, VersionedRouter};
use ryframe_middleware::{api_response_envelope_middleware, request_id_middleware};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

fn localizer() -> Arc<ryframe_i18n::Localizer> {
    Arc::new(ryframe_i18n::Localizer::embedded().expect("内嵌国际化资源应有效"))
}

#[test]
fn test_api_version_display() {
    assert_eq!(ApiVersion::v1().to_string(), "v1");
    assert_eq!(ApiVersion::v2().to_string(), "v2");
    assert_eq!(ApiVersion::new(10).to_string(), "v10");
}

#[test]
fn test_api_version_from_str() {
    assert_eq!("v1".parse::<ApiVersion>().unwrap(), ApiVersion::v1());
    assert_eq!("v2".parse::<ApiVersion>().unwrap(), ApiVersion::v2());
    assert_eq!("1".parse::<ApiVersion>().unwrap(), ApiVersion::v1());
    assert!("invalid".parse::<ApiVersion>().is_err());
}

#[test]
fn test_api_version_from_path() {
    assert_eq!(
        ApiVersion::from_path("/api/v1/users"),
        Some(ApiVersion::v1())
    );
    assert_eq!(
        ApiVersion::from_path("/api/v2/orders/123"),
        Some(ApiVersion::v2())
    );
    assert_eq!(ApiVersion::from_path("/other/path"), None);
    assert_eq!(ApiVersion::from_path("/api/noversion"), None);
}

#[test]
fn test_api_version_path_prefix() {
    assert_eq!(ApiVersion::v1().path_prefix(), "/api/v1");
    assert_eq!(ApiVersion::v2().path_prefix(), "/api/v2");
}

#[test]
fn test_api_version_ordering() {
    assert!(ApiVersion::v1() < ApiVersion::v2());
    assert!(ApiVersion::v2() > ApiVersion::v1());
    assert_eq!(ApiVersion::v1(), ApiVersion::v1());
}

#[test]
fn test_versioned_router_basic() {
    async fn v1_handler() -> Json<Value> {
        Json(json!({"version": "v1"}))
    }
    async fn v2_handler() -> Json<Value> {
        Json(json!({"version": "v2"}))
    }

    let v1 = Router::<()>::new().route("/test", get(v1_handler));
    let v2 = Router::<()>::new().route("/test", get(v2_handler));

    let _router = VersionedRouter::new().with_v1(v1).with_v2(v2).into_router();

    // Router 创建成功（无 panic 即通过）
}

#[test]
fn test_versioned_router_latest() {
    let router = VersionedRouter::<()>::new()
        .with_v1(Router::<()>::new())
        .with_v2(Router::<()>::new());

    assert_eq!(router.latest_version(), &ApiVersion::v2());
    assert!(router.has_version(&ApiVersion::v1()));
    assert!(router.has_version(&ApiVersion::v2()));
    assert!(!router.has_version(&ApiVersion::v3()));
    assert_eq!(router.registered_versions().len(), 2);
}

#[tokio::test]
async fn test_versioned_router_only_v1_rejects_v2_without_fallback() {
    async fn v1_handler() -> &'static str {
        "v1"
    }

    let router = VersionedRouter::new()
        .with_v1(Router::<()>::new().route("/version-check", get(v1_handler)))
        .into_router()
        .layer(from_fn_with_state(
            localizer(),
            api_response_envelope_middleware,
        ))
        .layer(from_fn(request_id_middleware));

    let v1_response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/version-check")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(v1_response.status(), StatusCode::OK);

    for uri in ["/api/v2/version-check", "/api/version-check"] {
        let response = router
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .expect("响应头必须包含请求 ID")
            .to_string();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(value["code"], 404);
        assert_eq!(value["message"], "资源不存在");
        assert_eq!(value["data"], Value::Null);
        assert_eq!(value["request_id"], request_id);
        assert_eq!(value["error_key"], "not_found");
        assert_eq!(value["details"], Value::Null);
    }
}

#[tokio::test]
async fn version_endpoint_uses_the_unified_response_contract() {
    let router = Router::new()
        .route("/api/v1/version", get(api_version))
        .layer(from_fn_with_state(
            localizer(),
            api_response_envelope_middleware,
        ))
        .layer(from_fn(request_id_middleware));

    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/version")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .expect("响应头必须包含请求 ID")
        .to_string();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(value["code"], 200);
    assert_eq!(value["request_id"], request_id);
    assert_eq!(value["data"]["name"], "ryframe-api");
    assert_eq!(value["data"]["api_prefix"], "/api/v1");
    assert!(value["data"]["endpoints"].is_object());
}
