use ryframe_service::system::online_user_service::{OnlineUserServiceImpl, UserSession};

fn make_session(token_id: &str, username: &str) -> UserSession {
    let now = chrono::Utc::now();
    UserSession {
        token_id: token_id.into(),
        user_id: 1,
        username: username.into(),
        dept_name: Some("研发部".into()),
        ipaddr: "192.168.1.1".into(),
        login_location: None,
        browser: Some("Chrome".into()),
        os: Some("Windows 10".into()),
        login_time: now,
        last_access_time: now,
    }
}

#[tokio::test]
async fn test_online_user_lifecycle() {
    let svc = OnlineUserServiceImpl::new_in_memory();

    // 添加用户
    svc.add_user(make_session("tok-1", "alice")).await;
    svc.add_user(make_session("tok-2", "bob")).await;
    assert_eq!(svc.count().await, 2);

    // 移除单个
    svc.remove_user("tok-1").await;
    assert_eq!(svc.count().await, 1);

    // 强制下线
    assert!(svc.force_logout("tok-2").await.is_ok());
    assert!(svc.force_logout("nonexistent").await.is_err());
    assert_eq!(svc.count().await, 0);
}

#[tokio::test]
async fn test_online_user_touch_and_cleanup() {
    let svc = OnlineUserServiceImpl::new_in_memory();

    // 添加过期用户 (60 分钟前)
    let mut old = make_session("tok-old", "olduser");
    old.last_access_time = chrono::Utc::now() - chrono::Duration::minutes(60);
    svc.add_user(old).await;
    svc.add_user(make_session("tok-new", "newuser")).await;

    // touch 更新
    svc.touch_user("tok-new").await;

    // 清理 30 分钟前的
    svc.cleanup_expired(30).await;
    let users = svc.list_online_users().await;
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].username, "newuser");
}
