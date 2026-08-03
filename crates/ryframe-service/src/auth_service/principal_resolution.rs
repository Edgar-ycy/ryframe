use async_trait::async_trait;
use ryframe_auth::{PrincipalResolver, RequestPrincipal, jwt::Claims};
use ryframe_core::Repository;
use ryframe_db::{ReadConsistency, entities::role};
use ryframe_kernel::{ActorContext, AppError, AppResult, DataScope, DataScopeContext};
use sea_orm::DatabaseConnection;

use crate::{AuthorizationSnapshot, AuthorizationVersions};

use super::{
    AuthService,
    identity::{AuthorizationProfile, ValidatedIdentity},
};

#[async_trait]
impl PrincipalResolver for AuthService {
    async fn resolve_principal(&self, claims: &Claims) -> AppResult<RequestPrincipal> {
        ryframe_core::validate_explicit_tenant(&claims.tenant_id)?;
        let user_id = claims
            .sub
            .parse::<i64>()
            .map_err(|_| AppError::Authentication("令牌中的用户ID无效".into()))?;

        let lookup = self
            .authorization_cache
            .lookup_snapshot(&claims.tenant_id, user_id)
            .await?;
        validate_mirrored_user_version(claims, lookup.user_authorization_version)?;
        if let Some(snapshot) = lookup.snapshot {
            return principal_from_snapshot(claims, user_id, snapshot);
        }

        for _ in 0..2 {
            let snapshot = self.rebuild_authorization_snapshot(claims).await?;
            if !self.authorization_cache.is_enabled()
                || self.authorization_cache.store_snapshot(&snapshot).await?
            {
                return Ok(snapshot.principal);
            }

            // 数据库读取期间发生了授权变更时，写脚本会拒绝旧版本；立即重读一次新快照。
            let lookup = self
                .authorization_cache
                .lookup_snapshot(&claims.tenant_id, user_id)
                .await?;
            validate_mirrored_user_version(claims, lookup.user_authorization_version)?;
            if let Some(snapshot) = lookup.snapshot {
                return principal_from_snapshot(claims, user_id, snapshot);
            }
        }

        Err(AppError::Authentication(
            "授权状态正在更新，请重新发起请求".into(),
        ))
    }
}

impl AuthService {
    async fn rebuild_authorization_snapshot(
        &self,
        claims: &Claims,
    ) -> AppResult<AuthorizationSnapshot> {
        let selected = self.db.select_read(ReadConsistency::Strong);
        let identity = self
            .validate_token_identity_on(&selected.connection, claims)
            .await?;
        let authorization = self
            .load_authorization_profile_on(
                &selected.connection,
                &identity.user.tenant_id,
                identity.user.id,
            )
            .await?;
        let data_scope = self
            .resolve_data_scope_on(
                &selected.connection,
                &identity.user.tenant_id,
                identity.user.id,
                identity.user.dept_id,
                &authorization.roles,
            )
            .await?;

        Ok(build_authorization_snapshot(
            &identity,
            authorization,
            data_scope,
        ))
    }

    async fn resolve_data_scope_on(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        user_id: i64,
        dept_id: Option<i64>,
        roles: &[role::Model],
    ) -> AppResult<DataScopeContext> {
        if roles.iter().any(|role| role.is_super == 1) {
            return Ok(DataScopeContext::super_admin(user_id));
        }

        let ancestors = match dept_id {
            Some(dept_id) => self
                .dept_repo
                .find_by_id(db, tenant_id, dept_id)
                .await?
                .map(|dept| dept.ancestors),
            None => None,
        };
        let custom_role_ids = roles
            .iter()
            .filter(|role| role.data_scope == role::Model::DATA_SCOPE_CUSTOM)
            .map(|role| role.id)
            .collect::<Vec<_>>();
        let custom_dept_ids = self
            .role_repo
            .find_roles_dept_ids(db, tenant_id, &custom_role_ids)
            .await?;
        let mut scopes = Vec::with_capacity(roles.len());

        for role in roles {
            let scope = DataScope::from_db_value(&role.data_scope);
            let scope_dept_ids = match scope {
                DataScope::Custom => custom_dept_ids.clone(),
                DataScope::Dept => dept_id.into_iter().collect(),
                DataScope::DeptAndChildren => match dept_id {
                    Some(dept_id) => {
                        self.dept_repo
                            .find_child_dept_ids(db, tenant_id, dept_id)
                            .await?
                    }
                    None => Vec::new(),
                },
                DataScope::All | DataScope::SelfOnly => Vec::new(),
            };
            scopes.push(DataScopeContext {
                scope,
                user_id,
                dept_id,
                ancestors: ancestors.clone(),
                custom_dept_ids: scope_dept_ids,
                include_self: false,
            });
        }

        if scopes.is_empty() {
            return Ok(DataScopeContext {
                scope: DataScope::SelfOnly,
                user_id,
                dept_id,
                ancestors,
                custom_dept_ids: Vec::new(),
                include_self: true,
            });
        }

        Ok(DataScopeContext::merge(scopes))
    }
}

fn build_authorization_snapshot(
    identity: &ValidatedIdentity,
    authorization: AuthorizationProfile,
    data_scope: DataScopeContext,
) -> AuthorizationSnapshot {
    let user = &identity.user;
    let is_super_admin = authorization.roles.iter().any(|role| role.is_super == 1);
    let role_ids = authorization.roles.iter().map(|role| role.id).collect();
    let roles = authorization
        .roles
        .iter()
        .map(|role| role.code.clone())
        .collect();

    AuthorizationSnapshot {
        versions: AuthorizationVersions {
            tenant_authorization_epoch: identity.tenant.authorization_epoch,
            user_authorization_version: user.authorization_version,
        },
        tenant_session_version: identity.tenant.session_version,
        principal: RequestPrincipal {
            actor: ActorContext {
                user_id: user.id,
                tenant_id: user.tenant_id.clone(),
                username: user.username.clone(),
                dept_id: user.dept_id,
                dept_path: data_scope.ancestors.clone(),
                data_scope: data_scope.scope,
                custom_dept_ids: data_scope.custom_dept_ids,
                include_self: data_scope.include_self,
                is_super_admin,
            },
            preferred_locale: user.preferred_locale.clone(),
            roles,
            role_ids,
            permissions: authorization.permissions,
            tenant_request_limit_per_minute: identity.tenant.max_requests_per_min.max(1) as u32,
        },
    }
}

fn validate_mirrored_user_version(
    claims: &Claims,
    user_authorization_version: Option<i32>,
) -> AppResult<()> {
    if user_authorization_version
        .is_some_and(|version| version != claims.user_authorization_version)
    {
        return Err(AppError::Authentication(
            "用户权限已变更，请重新登录".into(),
        ));
    }
    Ok(())
}

fn principal_from_snapshot(
    claims: &Claims,
    expected_user_id: i64,
    snapshot: AuthorizationSnapshot,
) -> AppResult<RequestPrincipal> {
    if snapshot.principal.actor.tenant_id != claims.tenant_id
        || snapshot.principal.actor.user_id != expected_user_id
        || snapshot.versions.user_authorization_version != claims.user_authorization_version
    {
        return Err(AppError::Authentication("授权快照身份不匹配".into()));
    }
    if snapshot.tenant_session_version != claims.tenant_session_version {
        return Err(AppError::Authentication(
            "租户会话已失效，请重新登录".into(),
        ));
    }
    Ok(snapshot.principal)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicI32, AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use ryframe_auth::{PrincipalResolver, RequestPrincipal, jwt::Claims};
    use ryframe_config::{
        AppConfig, AppSettings, AuthConfig, DatabaseConfig, DbConnection, Environment,
        LoggerConfig, RateLimitConfig,
    };
    use ryframe_db::{
        DatabaseCluster, DatabaseMetricsObserver, DatabaseNodeKind, DatabaseReadSelectionReason,
    };
    use ryframe_kernel::{ActorContext, AppError, DataScope};
    use sea_orm::{DbBackend, DbErr, MockDatabase};

    use crate::{
        AuthorizationCache, AuthorizationCacheBackend, AuthorizationCacheLookup,
        AuthorizationSnapshot, AuthorizationVersions, NamespaceCacheLookup, TenantCacheLookup,
    };

    use super::AuthService;

    #[derive(Debug)]
    struct CountingDatabaseObserver {
        strong_reads: AtomicUsize,
    }

    impl CountingDatabaseObserver {
        fn new() -> Self {
            Self {
                strong_reads: AtomicUsize::new(0),
            }
        }
    }

    impl DatabaseMetricsObserver for CountingDatabaseObserver {
        fn set_node_health(&self, _kind: DatabaseNodeKind, _name: &str, _healthy: bool) {}

        fn record_read_selection(
            &self,
            _target: DatabaseNodeKind,
            reason: DatabaseReadSelectionReason,
        ) {
            if reason == DatabaseReadSelectionReason::Strong {
                self.strong_reads.fetch_add(1, Ordering::SeqCst);
            }
        }

        fn record_read_fallback(&self) {}
    }

    struct CountingAuthorizationBackend {
        lookup_result: Mutex<Result<AuthorizationCacheLookup, String>>,
        lookup_calls: AtomicUsize,
        store_calls: AtomicUsize,
        mirrored_user_version: AtomicI32,
        user_mirror_calls: AtomicUsize,
        mirror_error: Mutex<Option<String>>,
    }

    impl CountingAuthorizationBackend {
        fn returning(result: Result<AuthorizationCacheLookup, String>) -> Self {
            Self {
                lookup_result: Mutex::new(result),
                lookup_calls: AtomicUsize::new(0),
                store_calls: AtomicUsize::new(0),
                mirrored_user_version: AtomicI32::new(0),
                user_mirror_calls: AtomicUsize::new(0),
                mirror_error: Mutex::new(None),
            }
        }

        fn fail_mirror_updates(&self, message: &str) {
            *self.mirror_error.lock().expect("授权镜像 Mock 锁不应中毒") = Some(message.into());
        }
    }

    #[async_trait]
    impl AuthorizationCacheBackend for CountingAuthorizationBackend {
        async fn lookup_snapshot(
            &self,
            _tenant_id: &str,
            _user_id: i64,
        ) -> Result<AuthorizationCacheLookup, String> {
            self.lookup_calls.fetch_add(1, Ordering::SeqCst);
            self.lookup_result
                .lock()
                .expect("授权缓存 Mock 锁不应中毒")
                .clone()
        }

        async fn store_snapshot(&self, _snapshot: &AuthorizationSnapshot) -> Result<bool, String> {
            self.store_calls.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        }

        async fn update_tenant_epoch(
            &self,
            _tenant_id: &str,
            _authorization_epoch: i32,
        ) -> Result<(), String> {
            if let Some(error) = self
                .mirror_error
                .lock()
                .expect("授权镜像 Mock 锁不应中毒")
                .clone()
            {
                return Err(error);
            }
            Ok(())
        }

        async fn update_user_version(
            &self,
            _tenant_id: &str,
            _user_id: i64,
            authorization_version: i32,
        ) -> Result<(), String> {
            if let Some(error) = self
                .mirror_error
                .lock()
                .expect("授权镜像 Mock 锁不应中毒")
                .clone()
            {
                return Err(error);
            }
            self.user_mirror_calls.fetch_add(1, Ordering::SeqCst);
            self.mirrored_user_version
                .fetch_max(authorization_version, Ordering::SeqCst);
            Ok(())
        }

        async fn read_tenant_value(
            &self,
            _tenant_id: &str,
            _namespace: &str,
        ) -> Result<Option<TenantCacheLookup>, String> {
            Ok(None)
        }

        async fn store_tenant_value(
            &self,
            _tenant_id: &str,
            _namespace: &str,
            _authorization_epoch: i32,
            _value: &str,
            _ttl_secs: u64,
        ) -> Result<bool, String> {
            Ok(true)
        }

        async fn update_namespace_version(
            &self,
            _tenant_id: &str,
            _namespace: &str,
            _namespace_version: i64,
        ) -> Result<(), String> {
            if let Some(error) = self
                .mirror_error
                .lock()
                .expect("授权镜像 Mock 锁不应中毒")
                .clone()
            {
                return Err(error);
            }
            Ok(())
        }

        async fn read_namespace_value(
            &self,
            _tenant_id: &str,
            _namespace: &str,
            _item: &str,
        ) -> Result<Option<NamespaceCacheLookup>, String> {
            Ok(Some(NamespaceCacheLookup {
                namespace_version: 0,
                value: None,
            }))
        }

        async fn store_namespace_value(
            &self,
            _tenant_id: &str,
            _namespace: &str,
            _item: &str,
            _namespace_version: i64,
            _value: &str,
            _ttl_secs: u64,
        ) -> Result<bool, String> {
            Ok(true)
        }
    }

    fn test_config() -> Arc<AppConfig> {
        Arc::new(AppConfig {
            environment: Environment::Test,
            snowflake_worker_id: 1,
            app: AppSettings {
                name: "authorization-cache-test".into(),
                port: 0,
                ..Default::default()
            },
            database: DatabaseConfig {
                primary: DbConnection {
                    database: "authorization_cache_test".into(),
                    max_connections: 1,
                    ..Default::default()
                },
                ..Default::default()
            },
            generator: Default::default(),
            auth: AuthConfig {
                jwt_secret: "authorization-cache-test-secret".into(),
                ..Default::default()
            },
            redis: None,
            logger: LoggerConfig::default(),
            rate_limit: RateLimitConfig::default(),
            pagination: Default::default(),
            cors: Default::default(),
            object_storage: Default::default(),
            proxy: Default::default(),
            upload: Default::default(),
            api_docs: Default::default(),
            monitor: Default::default(),
            jobs: Default::default(),
            telemetry: Default::default(),
            messaging: Default::default(),
        })
    }

    fn claims() -> Claims {
        Claims {
            sub: "42".into(),
            tenant_id: "tenant-a".into(),
            tenant_session_version: 3,
            user_authorization_version: 13,
            username: "alice".into(),
            token_type: "access".into(),
            sid: "session-a".into(),
            jti: "token-a".into(),
            iat: 1,
            exp: usize::MAX,
        }
    }

    fn snapshot() -> AuthorizationSnapshot {
        AuthorizationSnapshot {
            versions: AuthorizationVersions {
                tenant_authorization_epoch: 8,
                user_authorization_version: 13,
            },
            tenant_session_version: 3,
            principal: RequestPrincipal {
                actor: ActorContext {
                    user_id: 42,
                    tenant_id: "tenant-a".into(),
                    username: "alice".into(),
                    dept_id: Some(7),
                    dept_path: Some("0,1".into()),
                    data_scope: DataScope::Dept,
                    custom_dept_ids: vec![7],
                    include_self: false,
                    is_super_admin: false,
                },
                preferred_locale: Some("zh-CN".into()),
                roles: vec!["auditor".into()],
                role_ids: vec![5],
                permissions: vec!["system:user:list".into()],
                tenant_request_limit_per_minute: 600,
            },
        }
    }

    fn service_with_backend(
        backend: Arc<CountingAuthorizationBackend>,
        required: bool,
    ) -> (AuthService, Arc<CountingDatabaseObserver>) {
        let database = DatabaseCluster::single(
            MockDatabase::new(DbBackend::MySql)
                .append_query_errors([DbErr::Custom("预期的授权查询失败".into())])
                .into_connection(),
        );
        let observer = Arc::new(CountingDatabaseObserver::new());
        database.set_metrics_observer(observer.clone());
        let cache = AuthorizationCache::from_backend(backend, required);
        (
            AuthService::new(database, test_config(), None, cache),
            observer,
        )
    }

    #[tokio::test]
    async fn hot_snapshot_hit_performs_one_cache_read_and_zero_authorization_sql() {
        let snapshot = snapshot();
        let backend = Arc::new(CountingAuthorizationBackend::returning(Ok(
            AuthorizationCacheLookup {
                tenant_authorization_epoch: Some(8),
                user_authorization_version: Some(13),
                snapshot: Some(snapshot),
            },
        )));
        let (service, observer) = service_with_backend(backend.clone(), true);

        let principal = service.resolve_principal(&claims()).await.unwrap();

        assert_eq!(principal.actor.user_id, 42);
        assert_eq!(backend.lookup_calls.load(Ordering::SeqCst), 1);
        assert_eq!(backend.store_calls.load(Ordering::SeqCst), 0);
        assert_eq!(observer.strong_reads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn snapshot_miss_selects_primary_with_strong_consistency() {
        let backend = Arc::new(CountingAuthorizationBackend::returning(Ok(
            AuthorizationCacheLookup {
                tenant_authorization_epoch: None,
                user_authorization_version: None,
                snapshot: None,
            },
        )));
        let (service, observer) = service_with_backend(backend.clone(), false);

        let result = service.resolve_principal(&claims()).await;

        assert!(matches!(result, Err(AppError::Database(_))));
        assert_eq!(backend.lookup_calls.load(Ordering::SeqCst), 1);
        assert_eq!(observer.strong_reads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn required_cache_failure_fails_closed_before_database_selection() {
        let backend = Arc::new(CountingAuthorizationBackend::returning(Err(
            "Redis 不可达".into()
        )));
        let (service, observer) = service_with_backend(backend.clone(), true);

        let result = service.resolve_principal(&claims()).await;

        assert!(matches!(result, Err(AppError::ServiceUnavailable(_))));
        assert_eq!(backend.lookup_calls.load(Ordering::SeqCst), 1);
        assert_eq!(observer.strong_reads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn outbox_repair_contract_is_idempotent_and_never_moves_version_backwards() {
        let backend = Arc::new(CountingAuthorizationBackend::returning(Ok(
            AuthorizationCacheLookup {
                tenant_authorization_epoch: None,
                user_authorization_version: None,
                snapshot: None,
            },
        )));
        let cache = AuthorizationCache::from_backend(backend.clone(), true);

        for version in [5, 5, 3, 7, 7] {
            cache
                .repair_user_version("tenant-a", 42, version)
                .await
                .unwrap();
        }

        assert_eq!(backend.user_mirror_calls.load(Ordering::SeqCst), 5);
        assert_eq!(backend.mirrored_user_version.load(Ordering::SeqCst), 7);
    }

    #[tokio::test]
    async fn required_mirror_failure_blocks_mutation_success_response() {
        let backend = Arc::new(CountingAuthorizationBackend::returning(Ok(
            AuthorizationCacheLookup {
                tenant_authorization_epoch: None,
                user_authorization_version: None,
                snapshot: None,
            },
        )));
        backend.fail_mirror_updates("Redis 写入失败");
        let cache = AuthorizationCache::from_backend(backend, true);

        let result = cache.sync_user_versions("tenant-a", &[(42, 14)]).await;

        assert!(matches!(result, Err(AppError::ServiceUnavailable(_))));
    }
}
