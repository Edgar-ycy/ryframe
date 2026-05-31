use axum::{Json, Router, routing::get};
use ryframe_api::versioning::{ApiVersion, VersionedRouter};
use serde_json::json;

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
    async fn v1_handler() -> Json<serde_json::Value> {
        Json(json!({"version": "v1"}))
    }
    async fn v2_handler() -> Json<serde_json::Value> {
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
