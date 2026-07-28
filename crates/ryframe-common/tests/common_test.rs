/// 公共库公开接口测试。
use axum::response::IntoResponse;
use ryframe_common::{
    ApiPageResponse, ApiResponse, AppError, BusinessType, HttpAppError, KernelAppError, UserStatus,
};

#[test]
fn test_user_status_variants() {
    assert_eq!(UserStatus::Normal, UserStatus::Normal);
    assert_ne!(UserStatus::Normal, UserStatus::Disabled);
    assert!(UserStatus::Normal.can_login());
    assert!(!UserStatus::Disabled.can_login());
    assert!(!UserStatus::Locked.can_login());
}

#[test]
fn test_business_type_variants() {
    assert_eq!(BusinessType::Other, BusinessType::Other);
    assert_ne!(BusinessType::Query, BusinessType::Delete);
}

#[test]
fn test_api_response_creation() {
    let resp = ApiResponse::success(42);
    assert_eq!(resp.code, 200);
    assert_eq!(resp.data, Some(42));
}

#[test]
fn test_api_response_error() {
    let resp = ApiResponse::<()>::fail(500, "Internal Error", "internal");
    assert_eq!(resp.code, 500);
    assert_eq!(resp.message, "Internal Error");
}

#[test]
fn test_api_page_response_creation() {
    let resp = ApiPageResponse::new(vec![1, 2, 3], 100, 2, 10, 100, "ok");
    assert_eq!(resp.data.items, vec![1, 2, 3]);
    assert_eq!(resp.data.total, 100);
    assert_eq!(resp.data.total_pages, 10);
    assert_eq!(resp.code, 200);
}

#[test]
fn test_app_error_config() {
    let err = AppError::Config("test error".into());
    let msg = format!("{}", err);
    assert!(msg.contains("test error"));
}

#[test]
fn test_legacy_app_error_still_implements_into_response() {
    let response = AppError::NotFound("记录不存在".into()).into_response();
    assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

#[test]
fn test_kernel_error_uses_explicit_http_wrapper() {
    let response =
        HttpAppError::from(KernelAppError::ServiceUnavailable("依赖不可用".into())).into_response();
    assert_eq!(
        response.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
}
