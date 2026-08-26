use ryframe_kernel::{BusinessType, UserStatus};

#[test]
fn user_status_distinguishes_login_eligibility() {
    assert!(UserStatus::Normal.can_login());
    assert!(!UserStatus::Disabled.can_login());
    assert!(!UserStatus::Locked.can_login());
}

#[test]
fn business_operation_types_preserve_distinct_semantics() {
    assert_ne!(BusinessType::Other, BusinessType::Query);
    assert_ne!(BusinessType::Query, BusinessType::Delete);
}
