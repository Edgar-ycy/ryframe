use ryframe_application::ports::users::USER_STATUS_PENDING_ACTIVATION;
use ryframe_db::{application_ports::users::to_user_record, entities::user};

#[test]
fn user_mapping_exposes_only_application_fields() {
    let created_at = chrono::DateTime::<chrono::Utc>::UNIX_EPOCH;
    let record = to_user_record(user::Model {
        id: 17,
        tenant_id: "tenant-a".to_owned(),
        username: "alice".to_owned(),
        password_hash: "secret-hash".to_owned(),
        nickname: "Alice".to_owned(),
        email: "alice@example.com".to_owned(),
        phone: "13800000000".to_owned(),
        avatar: Some("avatar".to_owned()),
        avatar_file_id: Some(8),
        preferred_locale: Some("zh-CN".to_owned()),
        status: USER_STATUS_PENDING_ACTIVATION.to_owned(),
        authorization_version: 3,
        dept_id: Some(5),
        remark: Some("remark".to_owned()),
        login_ip: None,
        login_date: None,
        del_flag: user::Model::DEL_FLAG_NORMAL.to_owned(),
        created_at,
        updated_at: created_at,
    });

    assert_eq!(record.id, 17);
    assert_eq!(record.username, "alice");
    assert_eq!(record.nickname, "Alice");
    assert_eq!(record.dept_id, Some(5));
    assert_eq!(record.created_at, created_at);
}
