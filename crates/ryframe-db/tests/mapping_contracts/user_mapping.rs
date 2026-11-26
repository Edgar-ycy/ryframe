use ryframe_application::ports::users::USER_STATUS_PENDING_ACTIVATION;
use ryframe_db::{application_ports::users::to_user_record, entities::user};
use time::{
    Date, OffsetDateTime, UtcOffset,
    macros::{offset, time},
};

#[test]
fn user_mapping_exposes_only_application_fields() {
    let model = |created_at| user::Model {
        id: 17,
        tenant_id: "tenant-a".to_owned(),
        username: "alice".to_owned(),
        password_hash: "secret-hash".to_owned(),
        password_change_required: true,
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
    };
    for (nanos, serialized) in [
        (0, "1969-12-31T23:59:59Z"),
        (120_000_000, "1969-12-31T23:59:59.120Z"),
        (123_400_000, "1969-12-31T23:59:59.123400Z"),
        (123_456_700, "1969-12-31T23:59:59.123456700Z"),
    ] {
        let original = OffsetDateTime::from_unix_timestamp(-1)
            .unwrap()
            .replace_nanosecond(nanos)
            .unwrap()
            .to_offset(offset!(+05:30));
        let model = model(original);
        let encoded = serde_json::to_value(&model).unwrap();
        assert_eq!(encoded["created_at"], serialized);
        assert_eq!(encoded["updated_at"], serialized);
        assert!(encoded.get("password_hash").is_none());
        let record = to_user_record(model).unwrap();
        assert_eq!(record.id, 17);
        assert_eq!(record.username, "alice");
        assert_eq!(record.nickname, "Alice");
        assert_eq!(record.dept_id, Some(5));
        assert_eq!(record.created_at.offset(), UtcOffset::UTC);
        assert_eq!(record.created_at.unix_timestamp(), -1);
        assert_eq!(record.created_at.nanosecond(), nanos);
    }
    let invalid = OffsetDateTime::new_in_offset(Date::MAX, time!(23:59:59), offset!(-01:00));
    let error = to_user_record(model(invalid)).unwrap_err();
    assert!(matches!(error, ryframe_kernel::AppError::Database(message)
        if message.starts_with("读取用户创建时间:")));
}
