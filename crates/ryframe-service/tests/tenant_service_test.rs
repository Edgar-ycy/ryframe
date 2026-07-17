mod common;

use ryframe_common::{ActorContext, AppError, DataScope};
use ryframe_db::{DatabaseCluster, TenantRepository};
use ryframe_service::system::{CreateTenantParams, TenantService, UpdateTenantParams};

fn actor(tenant_id: &str, is_super_admin: bool) -> ActorContext {
    ActorContext {
        user_id: 1,
        tenant_id: tenant_id.into(),
        username: "admin".into(),
        dept_id: None,
        dept_path: None,
        data_scope: DataScope::All,
        custom_dept_ids: Vec::new(),
        include_self: true,
        is_super_admin,
    }
}

#[tokio::test]
async fn only_system_super_admin_can_manage_tenants() {
    let service = TenantService::new(DatabaseCluster::single(common::setup_test_db().await));

    assert!(service.list(&actor("tenant-b", true)).await.is_err());
    assert!(service.list(&actor("system", false)).await.is_err());
    assert_eq!(service.list(&actor("system", true)).await.unwrap().len(), 1);
}

#[tokio::test]
async fn tenant_creation_rejects_weak_admin_password() {
    let db = common::setup_test_db().await;
    let service = TenantService::new(DatabaseCluster::single(db.clone()));
    let error = service
        .create(
            &actor("system", true),
            CreateTenantParams {
                tenant_id: "tenant-weak".into(),
                name: "弱密码租户".into(),
                domain: None,
                expire_at: None,
                max_users: None,
                max_roles: None,
                max_storage_mb: None,
                max_requests_per_min: None,
                admin_username: "tenant-admin".into(),
                admin_password: "password123".into(),
            },
        )
        .await
        .expect_err("weak admin passwords must be rejected");

    assert!(matches!(error, AppError::Validation(_)));
    assert!(
        TenantRepository
            .find_by_tenant_id(&db, "tenant-weak")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn tenant_lifecycle_initializes_admin_and_invalidates_sessions() {
    let db = common::setup_test_db().await;
    let service = TenantService::new(DatabaseCluster::single(db.clone()));
    let platform_admin = actor("system", true);

    let created = service
        .create(
            &platform_admin,
            CreateTenantParams {
                tenant_id: "tenant-b".into(),
                name: "租户 B".into(),
                domain: None,
                expire_at: None,
                max_users: None,
                max_roles: None,
                max_storage_mb: None,
                max_requests_per_min: None,
                admin_username: "tenant-admin".into(),
                admin_password: "StrongPassword123!".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(created.tenant_id, "tenant-b");
    assert_eq!(created.max_users, 100);

    let user = ryframe_db::UserRepository
        .find_by_username(&db, "tenant-b", "tenant-admin")
        .await
        .unwrap()
        .expect("tenant admin should be created");
    assert!(ryframe_auth::password::verify("StrongPassword123!", &user.password_hash).unwrap());

    let expire_at = chrono::Utc::now() + chrono::Duration::days(30);
    service
        .update(
            &platform_admin,
            "tenant-b",
            UpdateTenantParams {
                name: "租户 B 已更新".into(),
                domain: Some("tenant-b.example.com".into()),
                expire_at: Some(expire_at),
                max_users: 200,
                max_roles: 30,
                max_storage_mb: 2048,
                max_requests_per_min: 2000,
            },
        )
        .await
        .unwrap();
    let updated = TenantRepository
        .find_by_tenant_id(&db, "tenant-b")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.session_version, 2);

    service
        .update_status(&platform_admin, "tenant-b", "0".into())
        .await
        .unwrap();
    let disabled = TenantRepository
        .find_by_tenant_id(&db, "tenant-b")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(disabled.status, "0");
    assert_eq!(disabled.session_version, 3);
    assert!(
        service
            .update_status(&platform_admin, "system", "0".into())
            .await
            .is_err()
    );
}
