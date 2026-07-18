/// ryframe-common 公开 API 测试
use ryframe_common::{ApiPageResponse, ApiResponse, AppError, BusinessType, UserStatus};

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
    let resp = ApiResponse::<()>::fail(500, "Internal Error".into());
    assert_eq!(resp.code, 500);
    assert_eq!(resp.msg, "Internal Error");
}

#[test]
fn test_api_page_response_creation() {
    let resp = ApiPageResponse::new(vec![1, 2, 3], 100, "ok");
    assert_eq!(resp.rows, vec![1, 2, 3]);
    assert_eq!(resp.total, Some(100));
    assert_eq!(resp.code, 200);
}

#[test]
fn test_app_error_config() {
    let err = AppError::Config("test error".into());
    let msg = format!("{}", err);
    assert!(msg.contains("test error"));
}
