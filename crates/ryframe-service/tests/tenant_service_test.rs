mod common;

use ryframe_db::{
    CONFIG_CACHE_NAMESPACE, CacheNamespaceVersionRepository, DatabaseCluster, RoleRepository,
    TenantRepository, UserRepository, entities::permission,
};
use ryframe_kernel::{ActorContext, AppError, DataScope};
use ryframe_service::system::{
    CreateTenantParams, CreateUserParams, RoleService, TenantService, UpdateTenantParams,
    UserService,
};
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};

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
    let db = common::setup_test_db().await;
    let service = TenantService::new(
        DatabaseCluster::single(db.connection().clone()),
        ryframe_service::AuthorizationCache::disabled(),
    );

    assert!(service.list(&actor("tenant-b", true)).await.is_err());
    assert!(service.list(&actor("system", false)).await.is_err());
    assert_eq!(service.list(&actor("system", true)).await.unwrap().len(), 1);
}

#[tokio::test]
async fn tenant_creation_rejects_weak_admin_password() {
    let db = common::setup_test_db().await;
    let service = TenantService::new(
        DatabaseCluster::single(db.clone()),
        ryframe_service::AuthorizationCache::disabled(),
    );
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
async fn tenant_creation_rejects_redis_glob_identifiers() {
    let db = common::setup_test_db().await;
    let service = TenantService::new(
        DatabaseCluster::single(db.clone()),
        ryframe_service::AuthorizationCache::disabled(),
    );
    for tenant_id in ["**", "a?", "a[b]", "a:b", "a\\b"] {
        let error = service
            .create(
                &actor("system", true),
                CreateTenantParams {
                    tenant_id: tenant_id.into(),
                    name: "Unsafe tenant".into(),
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
            .expect_err("Redis glob characters must be rejected");
        assert!(matches!(error, AppError::Validation(_)), "{tenant_id}");
    }
}

#[tokio::test]
async fn tenant_creation_rejects_limits_below_provisioned_resources() {
    let db = common::setup_test_db().await;
    let service = TenantService::new(
        DatabaseCluster::single(db.clone()),
        ryframe_service::AuthorizationCache::disabled(),
    );
    let error = service
        .create(
            &actor("system", true),
            CreateTenantParams {
                tenant_id: "tenant-too-small".into(),
                name: "Too Small".into(),
                domain: None,
                expire_at: None,
                max_users: Some(0),
                max_roles: Some(1),
                max_storage_mb: Some(0),
                max_requests_per_min: Some(0),
                admin_username: "tenant-admin".into(),
                admin_password: "StrongPassword123!".into(),
            },
        )
        .await
        .expect_err("provisioning minimums must be enforced");

    assert!(matches!(error, AppError::Validation(_)));
    assert!(
        TenantRepository
            .find_by_tenant_id(&db, "tenant-too-small")
            .await
            .expect("query tenant")
            .is_none()
    );
}

#[tokio::test]
async fn concurrent_user_creation_cannot_exceed_tenant_quota() {
    let db = common::setup_test_db().await;
    let platform_admin = actor("system", true);
    let cluster = DatabaseCluster::single(db.clone());
    let tenant_service = TenantService::new(
        cluster.clone(),
        ryframe_service::AuthorizationCache::disabled(),
    );
    tenant_service
        .update(
            &platform_admin,
            "system",
            UpdateTenantParams {
                name: "系统租户".into(),
                domain: None,
                expire_at: None,
                max_users: 1,
                max_roles: 20,
                max_storage_mb: 1024,
                max_requests_per_min: 1000,
            },
        )
        .await
        .expect("set one-user quota");

    let user_service = UserService::new(cluster, ryframe_service::AuthorizationCache::disabled());
    let (first, second) = tokio::join!(
        user_service.create(
            &platform_admin,
            CreateUserParams {
                username: "quota-first",
                nickname: "Quota First",
                email: "first@example.com",
                phone: "10001",
                dept_id: None,
                role_ids: Vec::new(),
            },
        ),
        user_service.create(
            &platform_admin,
            CreateUserParams {
                username: "quota-second",
                nickname: "Quota Second",
                email: "second@example.com",
                phone: "10002",
                dept_id: None,
                role_ids: Vec::new(),
            },
        ),
    );

    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let first_saved = UserRepository
        .find_by_username(&db, "system", "quota-first")
        .await
        .expect("query first user")
        .is_some();
    let second_saved = UserRepository
        .find_by_username(&db, "system", "quota-second")
        .await
        .expect("query second user")
        .is_some();
    assert_ne!(first_saved, second_saved);
}

#[tokio::test]
async fn concurrent_role_creation_cannot_exceed_tenant_quota() {
    let db = common::setup_test_db().await;
    let platform_admin = actor("system", true);
    let cluster = DatabaseCluster::single(db.clone());
    RoleService::new(
        cluster.clone(),
        ryframe_service::AuthorizationCache::disabled(),
    )
    .create(&platform_admin, "Existing Role", "quota-existing", 0, None)
    .await
    .expect("seed existing role");
    let tenant_service = TenantService::new(
        cluster.clone(),
        ryframe_service::AuthorizationCache::disabled(),
    );
    tenant_service
        .update(
            &platform_admin,
            "system",
            UpdateTenantParams {
                name: "系统租户".into(),
                domain: None,
                expire_at: None,
                max_users: 100,
                max_roles: 2,
                max_storage_mb: 1024,
                max_requests_per_min: 1000,
            },
        )
        .await
        .expect("leave one role slot");

    let role_service = RoleService::new(cluster, ryframe_service::AuthorizationCache::disabled());
    let (first, second) = tokio::join!(
        role_service.create(&platform_admin, "Quota First", "quota-first", 1, None),
        role_service.create(&platform_admin, "Quota Second", "quota-second", 2, None),
    );

    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let first_saved = RoleRepository
        .find_by_code(&db, "system", "quota-first")
        .await
        .expect("query first role")
        .is_some();
    let second_saved = RoleRepository
        .find_by_code(&db, "system", "quota-second")
        .await
        .expect("query second role")
        .is_some();
    assert_ne!(first_saved, second_saved);
}

#[tokio::test]
async fn tenant_limits_cannot_be_lowered_below_current_usage() {
    let db = common::setup_test_db().await;
    let platform_admin = actor("system", true);
    let cluster = DatabaseCluster::single(db.clone());
    let user_service = UserService::new(
        cluster.clone(),
        ryframe_service::AuthorizationCache::disabled(),
    );
    for (username, phone) in [("usage-first", "20001"), ("usage-second", "20002")] {
        user_service
            .create(
                &platform_admin,
                CreateUserParams {
                    username,
                    nickname: username,
                    email: "usage@example.com",
                    phone,
                    dept_id: None,
                    role_ids: Vec::new(),
                },
            )
            .await
            .expect("seed current user usage");
    }

    let error = TenantService::new(cluster, ryframe_service::AuthorizationCache::disabled())
        .update(
            &platform_admin,
            "system",
            UpdateTenantParams {
                name: "系统租户".into(),
                domain: None,
                expire_at: None,
                max_users: 1,
                max_roles: 2,
                max_storage_mb: 1024,
                max_requests_per_min: 1000,
            },
        )
        .await
        .expect_err("limits below current usage must be rejected");

    assert!(matches!(error, AppError::Validation(_)));
    let tenant = TenantRepository
        .find_by_tenant_id(&db, "system")
        .await
        .expect("query tenant")
        .expect("tenant exists");
    assert_eq!(tenant.max_users, 100);
}

#[tokio::test]
async fn tenant_lifecycle_initializes_admin_and_invalidates_sessions() {
    let db = common::setup_test_db().await;
    let service = TenantService::new(
        DatabaseCluster::single(db.clone()),
        ryframe_service::AuthorizationCache::disabled(),
    );
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

    let user = UserRepository
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

#[tokio::test]
async fn tenant_provisioning_does_not_copy_platform_permissions() {
    let db = common::setup_test_db().await;
    let now = chrono::Utc::now();
    for (id, code) in [
        (1001, "system:message:publish"),
        (1002, "platform:message:publish"),
    ] {
        permission::ActiveModel {
            id: ActiveValue::Set(id),
            tenant_id: ActiveValue::Set("system".into()),
            name: ActiveValue::Set(code.into()),
            code: ActiveValue::Set(code.into()),
            parent_id: ActiveValue::Set(None),
            perm_type: ActiveValue::Set("api".into()),
            icon: ActiveValue::Set(None),
            sort: ActiveValue::Set(0),
            status: ActiveValue::Set("1".into()),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        }
        .insert(&db)
        .await
        .expect("seed system permission");
    }

    TenantService::new(
        DatabaseCluster::single(db.clone()),
        ryframe_service::AuthorizationCache::disabled(),
    )
    .create(
        &actor("system", true),
        CreateTenantParams {
            tenant_id: "tenant-no-platform-permission".into(),
            name: "权限隔离租户".into(),
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
    .expect("provision tenant");

    assert_eq!(
        CacheNamespaceVersionRepository
            .find_version(&db, "tenant-no-platform-permission", CONFIG_CACHE_NAMESPACE,)
            .await
            .expect("查询新租户缓存命名空间版本"),
        0
    );

    let copied_codes = permission::Entity::find()
        .filter(permission::Column::TenantId.eq("tenant-no-platform-permission"))
        .all(&db)
        .await
        .expect("query provisioned permissions")
        .into_iter()
        .map(|permission| permission.code)
        .collect::<Vec<_>>();
    assert!(
        copied_codes
            .iter()
            .any(|code| code == "system:message:publish")
    );
    assert!(
        !copied_codes
            .iter()
            .any(|code| code == "platform:message:publish")
    );
}
