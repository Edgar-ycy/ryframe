use ryframe_common::{ActorContext, DataScope};
use ryframe_service::system::online_user_service::{OnlineUserService, UserSession};

fn actor(tenant_id: &str) -> ActorContext {
    ActorContext {
        user_id: 1,
        tenant_id: tenant_id.into(),
        username: "admin".into(),
        dept_id: None,
        dept_path: None,
        data_scope: DataScope::All,
        custom_dept_ids: Vec::new(),
        include_self: true,
        is_super_admin: true,
    }
}

fn make_session(sid: &str, username: &str) -> UserSession {
    make_tenant_session("system", sid, username)
}

fn make_tenant_session(tenant_id: &str, sid: &str, username: &str) -> UserSession {
    let now = chrono::Utc::now();
    UserSession {
        sid: sid.into(),
        tenant_id: tenant_id.into(),
        user_id: 1,
        username: username.into(),
        dept_name: Some("研发部".into()),
        ipaddr: "192.168.1.1".into(),
        login_location: None,
        browser: Some("Chrome".into()),
        os: Some("Windows 10".into()),
        login_time: now,
        last_access_time: now,
        absolute_exp: (now + chrono::Duration::days(7)).timestamp(),
    }
}

#[tokio::test]
async fn online_users_are_isolated_by_tenant() {
    let svc = OnlineUserService::new_in_memory();
    svc.add_user(make_tenant_session("tenant-b", "shared-token", "bob"))
        .await;

    assert!(
        svc.list_online_users(&actor("system"))
            .await
            .unwrap()
            .is_empty()
    );
    svc.remove_user("system", "shared-token").await;
    assert_eq!(svc.count(&actor("tenant-b")).await.unwrap(), 1);
    svc.remove_user("tenant-b", "shared-token").await;
    assert_eq!(svc.count(&actor("tenant-b")).await.unwrap(), 0);
}

#[tokio::test]
async fn test_online_user_lifecycle() {
    let svc = OnlineUserService::new_in_memory();

    // 添加用户
    svc.add_user(make_session("tok-1", "alice")).await;
    svc.add_user(make_session("tok-2", "bob")).await;
    assert_eq!(svc.count(&actor("system")).await.unwrap(), 2);

    // 移除单个
    svc.remove_user("system", "tok-1").await;
    assert_eq!(svc.count(&actor("system")).await.unwrap(), 1);

    svc.remove_user("system", "tok-2").await;
    // Secondary-index deletion is deliberately idempotent.
    svc.remove_user("system", "nonexistent").await;
    assert_eq!(svc.count(&actor("system")).await.unwrap(), 0);
}

#[tokio::test]
async fn test_online_user_touch_and_cleanup() {
    let svc = OnlineUserService::new_in_memory();

    // 添加一个 refresh family 已过绝对期限的索引条目。
    let mut old = make_session("tok-old", "olduser");
    old.absolute_exp = chrono::Utc::now().timestamp() - 1;
    svc.add_user(old).await;
    svc.add_user(make_session("tok-new", "newuser")).await;

    // touch 更新
    svc.touch_user("system", "tok-new").await;

    svc.cleanup_expired().await;
    let users = svc.list_online_users(&actor("system")).await.unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].username, "newuser");
}
