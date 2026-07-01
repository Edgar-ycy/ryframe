use chrono::Utc;
use ryframe_db::entities::user;

fn make_user(status: &str) -> user::Model {
    let now = Utc::now();
    user::Model {
        id: 1,
        tenant_id: "system".to_string(),
        username: "test_user".to_string(),
        password_hash: "hash".to_string(),
        nickname: "Test User".to_string(),
        email: "test@example.com".to_string(),
        phone: "13800000000".to_string(),
        avatar: None,
        status: status.to_string(),
        auth_version: 1,
        dept_id: None,
        remark: None,
        login_ip: None,
        login_date: None,
        del_flag: user::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now,
        updated_at: now,
    }
}

#[test]
fn user_status_constants_remain_stable() {
    assert_eq!(user::Model::STATUS_DISABLED, "0");
    assert_eq!(user::Model::STATUS_NORMAL, "1");
    assert_eq!(user::Model::STATUS_LOCKED, "2");
    assert_eq!(user::Model::STATUS_PENDING_ACTIVATION, "pending_activation");
    assert_eq!(
        user::Model::STATUS_MUST_RESET_PASSWORD,
        "must_reset_password"
    );
}

#[test]
fn user_enabled_requires_normal_status() {
    assert!(make_user(user::Model::STATUS_NORMAL).is_enabled());
    assert!(!make_user(user::Model::STATUS_DISABLED).is_enabled());
    assert!(!make_user(user::Model::STATUS_LOCKED).is_enabled());
    assert!(!make_user(user::Model::STATUS_PENDING_ACTIVATION).is_enabled());
    assert!(!make_user(user::Model::STATUS_MUST_RESET_PASSWORD).is_enabled());
}
