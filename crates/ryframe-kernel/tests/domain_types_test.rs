use ryframe_kernel::{BusinessType, UserStatus};

#[test]
fn 用户状态明确区分可登录与不可登录状态() {
    assert!(UserStatus::Normal.can_login());
    assert!(!UserStatus::Disabled.can_login());
    assert!(!UserStatus::Locked.can_login());
}

#[test]
fn 业务操作类型保留独立语义() {
    assert_ne!(BusinessType::Other, BusinessType::Query);
    assert_ne!(BusinessType::Query, BusinessType::Delete);
}
