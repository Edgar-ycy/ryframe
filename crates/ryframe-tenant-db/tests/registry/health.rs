use super::*;
use std::error::Error;

#[tokio::test]
async fn cached_schema_and_fresh_ping_failures_keep_safe_causes_and_health() {
    let pool = Arc::new(OpenPool::new(
        DatabaseConnection::Disconnected,
        ryframe_config::DbConnection {
            password: "private-health-password".into(),
            ..Default::default()
        },
        1,
        Arc::new(Mutex::new(TargetHealthRecord::default())),
    ));
    let lease = pool.acquire(Arc::from("health-target"));
    // 独立覆盖 OnceCell 的错误克隆与公开健康状态，不让 Disconnected 冒充真实 schema 查询。
    let schema_error = lease.unavailable("schema", "schema drift private-health-password");
    pool.schema_verification.set(Err(schema_error)).unwrap();
    let first = lease.ensure_schema_verified().await.unwrap_err();
    let cached = lease.ensure_schema_verified().await.unwrap_err();
    assert_eq!(
        first.source().unwrap().to_string(),
        cached.source().unwrap().to_string()
    );
    assert!(!lease.is_schema_verified());
    assert!(matches!(pool.schema_verification.get(), Some(Err(_))));
    let catalog = crate::migration::TENANT_DATA_CATALOG;
    let fresh = lease
        .verify_schema_now_for_catalog(&catalog)
        .await
        .unwrap_err();
    assert!(
        fresh
            .source()
            .unwrap()
            .to_string()
            .contains("数据库健康检查失败")
    );
    for error in [&first, &cached, &fresh] {
        assert_eq!(error.to_string(), "租户数据目标 health-target 暂不可用");
        assert!(!format!("{error:?}").contains("private-health-password"));
    }
    let health = pool.health.lock().unwrap();
    assert_eq!(health.status, TenantDatabaseTargetHealthStatus::Unavailable);
    assert!(health.last_verified_at.is_some());
}
