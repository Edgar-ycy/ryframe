use chrono::Utc;
use ryframe_application::{
    AuditRequestContext, bind_current_audit, scope_audit_request,
    system::{
        InMemoryOnlineSessionMetadata, OnlineSessionMetadataStore, OperLogStatus,
        RecordOperLogCommand, UserSession,
    },
};

fn audit_command() -> RecordOperLogCommand {
    RecordOperLogCommand {
        title: "更新用户".to_owned(),
        business_type: "update".to_owned(),
        method: "handler".to_owned(),
        request_method: "PUT".to_owned(),
        oper_name: "admin".to_owned(),
        oper_url: "/users/1".to_owned(),
        oper_ip: "127.0.0.1".to_owned(),
        oper_param: None,
        json_result: None,
        status: OperLogStatus::Failure,
        error_msg: Some("尚未完成".to_owned()),
        cost_time: 0,
    }
}

fn session(sid: &str, absolute_exp: i64) -> UserSession {
    let now = Utc::now();
    UserSession {
        sid: sid.into(),
        tenant_id: "tenant-a".into(),
        user_id: 42,
        username: "alice".into(),
        dept_name: None,
        ipaddr: "192.0.2.1".into(),
        login_location: None,
        browser: None,
        os: None,
        login_time: now,
        last_access_time: now,
        absolute_exp,
    }
}

#[tokio::test]
async fn transaction_binding_marks_attempt_and_commit_separately() {
    let context = AuditRequestContext::new(
        "event-1".to_owned(),
        "request-1".to_owned(),
        "tenant-a".to_owned(),
        audit_command(),
    )
    .expect("审计上下文应创建成功");
    let observed = context.clone();

    scope_audit_request(context, async {
        let (event, binding) = bind_current_audit().expect("审计上下文应存在");
        assert_eq!(event.event_id, "event-1");
        assert!(observed.transaction_bound());
        assert!(!observed.transaction_committed());
        binding.mark_committed();
    })
    .await;

    assert!(observed.transaction_committed());
}

#[tokio::test]
async fn online_metadata_is_isolated_and_removable() {
    let store = InMemoryOnlineSessionMetadata::default();
    store
        .add(session("sid-a", Utc::now().timestamp() + 60), 60)
        .await
        .expect("应写入设备元数据");
    assert_eq!(
        store
            .list_for_user("tenant-a", 42)
            .await
            .expect("应读取用户设备")
            .len(),
        1
    );
    assert!(
        store
            .list("tenant-b")
            .await
            .expect("应隔离其他租户")
            .is_empty()
    );
    assert!(
        store
            .touch("tenant-a", "sid-a")
            .await
            .expect("应更新设备活动时间")
    );
    store
        .remove("tenant-a", "sid-a")
        .await
        .expect("应删除设备元数据");
    assert!(
        store
            .list("tenant-a")
            .await
            .expect("应读取租户设备")
            .is_empty()
    );
}

#[tokio::test]
async fn expired_online_metadata_is_not_returned() {
    let store = InMemoryOnlineSessionMetadata::default();
    store
        .add(session("sid-expired", Utc::now().timestamp() - 1), 1)
        .await
        .expect("应允许写入待清理元数据");
    store.cleanup_expired().await.expect("应清理过期元数据");
    assert!(
        store
            .list("tenant-a")
            .await
            .expect("应读取租户设备")
            .is_empty()
    );
}
