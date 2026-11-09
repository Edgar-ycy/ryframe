use ryframe_kernel::UserStatus;

#[test]
fn user_status_distinguishes_login_eligibility() {
    assert!(UserStatus::Normal.can_login());
    assert!(!UserStatus::Disabled.can_login());
    assert!(!UserStatus::Locked.can_login());
}
