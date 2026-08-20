use std::sync::Arc;

use ryframe_adapters::{RedisClient, rate_limit::RateLimiter};
use ryframe_api::AppServices;
use ryframe_application::{
    ArtifactStore, AuditOutbox, AuthService, AuthorizationCache, JobQueue, JobScheduleService,
    TenantBusinessDataState, TenantRuntimeReadFuture, TenantRuntimeReadPort, TenantRuntimeSnapshot,
    agent::{AgentService, service_capability_descriptors},
    map_tenant_data_error,
    system::{
        AuthorizationDiagnosticService, CaptchaStore, CaptchaStoreFuture, ConfigService,
        DataRetentionService, DeptService, DictCacheStore, DictCacheStoreFuture, DictService,
        ExportService, FileService, InMemoryCaptchaStore, LoginInfoService, MenuService,
        MessageService, NoticeService, OnlineUserService, OperLogService, OverviewService,
        PermissionService, PostService, ProductService, ProfileService, RoleService,
        ServiceAccountService, TenantConfigTransferService, TenantDataMigrationService,
        TenantRateLimitReadFuture, TenantRateLimitReadPort, TenantRateLimitSnapshot, TenantService,
        TenantUsageService, UserImportService, UserService, WebSocketTicketService,
        WebSocketTicketStore, WebSocketTicketStoreFuture,
    },
};
use ryframe_config::AppConfig;
use ryframe_db::ControlDatabaseCluster;
use ryframe_kernel::{AppError, CAPTCHA_KEY_PREFIX};
use ryframe_tenant_db::{TenantDataState, TenantDatabaseRouter};

use super::application_policy::{ApplicationPolicies, load_pepper_keyring};

struct TenantRuntimeReader {
    router: Arc<TenantDatabaseRouter>,
}

impl TenantRuntimeReadPort for TenantRuntimeReader {
    fn runtime_snapshot<'a>(&'a self, tenant_id: &'a str) -> TenantRuntimeReadFuture<'a> {
        Box::pin(async move {
            let snapshot = self
                .router
                .runtime_snapshot(tenant_id)
                .await
                .map_err(map_tenant_data_error)?;
            let state = match snapshot.business_data_state() {
                TenantDataState::Provisioning => TenantBusinessDataState::Provisioning,
                TenantDataState::Active => TenantBusinessDataState::Active,
                TenantDataState::Maintenance => TenantBusinessDataState::Maintenance,
                TenantDataState::Failed => TenantBusinessDataState::Failed,
            };
            Ok(TenantRuntimeSnapshot::new(
                snapshot.tenant_id().to_owned(),
                snapshot.authorization_epoch(),
                snapshot.runtime_epoch(),
                snapshot.placement_generation(),
                state,
            ))
        })
    }
}

struct TenantRateLimitReader {
    limiter: Arc<RateLimiter>,
}

struct RedisWebSocketTicketStore {
    client: RedisClient,
}

struct RedisCaptchaStore {
    client: RedisClient,
    ttl_secs: u64,
}

struct RedisDictCacheStore {
    client: RedisClient,
}

impl DictCacheStore for RedisDictCacheStore {
    fn get<'a>(&'a self, key: &'a str) -> DictCacheStoreFuture<'a, Option<String>> {
        Box::pin(async move {
            self.client
                .get(key)
                .await
                .map_err(|error| AppError::ServiceUnavailable(error.to_string()))
        })
    }

    fn put(&self, key: String, value: String, ttl_secs: u64) -> DictCacheStoreFuture<'_, ()> {
        Box::pin(async move {
            self.client
                .set_ex(key, value, ttl_secs)
                .await
                .map_err(|error| AppError::ServiceUnavailable(error.to_string()))
        })
    }

    fn remove(&self, key: String) -> DictCacheStoreFuture<'_, ()> {
        Box::pin(async move {
            self.client
                .del(key)
                .await
                .map(|_| ())
                .map_err(|error| AppError::ServiceUnavailable(error.to_string()))
        })
    }
}

impl CaptchaStore for RedisCaptchaStore {
    fn set(&self, id: String, answer: String) -> CaptchaStoreFuture<'_, ()> {
        Box::pin(async move {
            let key = format!("{CAPTCHA_KEY_PREFIX}{id}");
            self.client
                .set_ex(key, answer, self.ttl_secs)
                .await
                .map_err(|error| {
                    tracing::error!(%error, "Redis SET 验证码失败");
                    AppError::ServiceUnavailable("验证码服务暂不可用".into())
                })
        })
    }

    fn verify<'a>(&'a self, id: &'a str, code: &'a str) -> CaptchaStoreFuture<'a, bool> {
        Box::pin(async move {
            let key = format!("{CAPTCHA_KEY_PREFIX}{id}");
            self.client
                .get_and_del(&key)
                .await
                .map(|stored| stored.is_some_and(|value| value.eq_ignore_ascii_case(code)))
                .map_err(|error| {
                    tracing::error!(%error, "Redis GETDEL 验证码失败");
                    AppError::ServiceUnavailable("验证码服务暂不可用".into())
                })
        })
    }
}

impl WebSocketTicketStore for RedisWebSocketTicketStore {
    fn put(&self, key: String, value: String, ttl_secs: u64) -> WebSocketTicketStoreFuture<'_, ()> {
        Box::pin(async move {
            self.client
                .set_ex(key, value, ttl_secs)
                .await
                .map_err(|error| {
                    AppError::ServiceUnavailable(format!("WebSocket 票据写入失败: {error}"))
                })
        })
    }

    fn take<'a>(&'a self, key: &'a str) -> WebSocketTicketStoreFuture<'a, Option<String>> {
        Box::pin(async move {
            self.client.get_and_del(key).await.map_err(|error| {
                AppError::ServiceUnavailable(format!("WebSocket 票据校验失败: {error}"))
            })
        })
    }
}

impl TenantRateLimitReadPort for TenantRateLimitReader {
    fn snapshot_many<'a>(&'a self, tenant_ids: &'a [String]) -> TenantRateLimitReadFuture<'a> {
        Box::pin(async move {
            let keys = tenant_ids
                .iter()
                .map(|tenant_id| RateLimiter::tenant_key(tenant_id))
                .collect::<Vec<_>>();
            self.limiter
                .snapshot_many(&keys, 1)
                .await
                .map_err(AppError::ServiceUnavailable)
                .map(|snapshots| {
                    snapshots
                        .into_iter()
                        .map(|snapshot| TenantRateLimitSnapshot {
                            current: snapshot.current,
                            remaining_secs: snapshot.remaining_secs,
                        })
                        .collect()
                })
        })
    }
}

/// 构造所有 Service 实例
///
/// 依赖注入顺序：Repository → Redis → Service。
pub async fn build_all(
    database: &ControlDatabaseCluster,
    tenant_data: Arc<TenantDatabaseRouter>,
    config: &AppConfig,
    policies: &ApplicationPolicies,
    redis_client: &Option<RedisClient>,
    object_storage: Arc<dyn ArtifactStore>,
    rate_limiter: Arc<RateLimiter>,
) -> Result<AppServices, AppError> {
    let authorization_cache = AuthorizationCache::new(redis_client.clone(), policies.cache);
    let user = Arc::new(UserService::new(
        database.clone(),
        authorization_cache.clone(),
    ));
    let product = Arc::new(ProductService::new(
        database.clone(),
        authorization_cache.clone(),
        policies.service_accounts.enabled() && redis_client.is_some(),
    ));
    let role = Arc::new(RoleService::new(
        database.clone(),
        authorization_cache.clone(),
        product.clone(),
    ));
    let tenant = Arc::new(TenantService::new(
        database.clone(),
        authorization_cache.clone(),
        product.clone(),
        tenant_data.clone(),
    ));
    let tenant_usage = Arc::new(TenantUsageService::new(
        database.clone(),
        Arc::new(TenantRateLimitReader {
            limiter: rate_limiter,
        }),
        config.rate_limit.enabled,
        policies.job_schedule.enabled,
    ));
    let (service_accounts, agent) = if policies.service_accounts.enabled() {
        let redis = redis_client.clone().ok_or_else(|| {
            AppError::Config("启用服务账号后必须配置 Redis，以保证 Agent 多实例限流一致".into())
        })?;
        let keyring = load_pepper_keyring(config)?;
        let descriptors = service_capability_descriptors();
        let management = Arc::new(ServiceAccountService::new(
            database.clone(),
            policies.service_accounts,
            Arc::clone(&keyring),
            descriptors,
            authorization_cache.clone(),
        )?);
        let agent = Arc::new(AgentService::new(
            database.clone(),
            redis,
            keyring,
            policies.service_accounts,
            policies.multi_tenancy,
            product.clone(),
        )?);
        (Some(management), Some(agent))
    } else {
        (None, None)
    };
    let permission = Arc::new(PermissionService::new(
        database.clone(),
        authorization_cache.clone(),
        product.clone(),
    ));
    let token_settings = Arc::new(ryframe_auth::jwt::TokenSettings::new(
        Arc::<str>::from(config.auth.jwt_secret.as_str()),
        &config.auth.access_token_expire,
        &config.auth.refresh_token_expire,
    )?);
    let auth = Arc::new(AuthService::new(
        database.clone(),
        policies.auth,
        token_settings,
        redis_client.clone(),
        authorization_cache.clone(),
    ));
    let menu = Arc::new(MenuService::new(
        database.clone(),
        authorization_cache.clone(),
    ));

    let dept = Arc::new(DeptService::new(
        database.clone(),
        authorization_cache.clone(),
    ));
    let post = Arc::new(PostService::new(database.clone()));
    let config_service = Arc::new(ConfigService::new(
        database.clone(),
        authorization_cache.clone(),
    ));

    let dict = Arc::new(DictService::new(
        database.clone(),
        redis_client.as_ref().map(|client| {
            Arc::new(RedisDictCacheStore {
                client: client.clone(),
            }) as Arc<dyn DictCacheStore>
        }),
    ));
    let notice = Arc::new(NoticeService::new(database.clone()));
    let oper_log = Arc::new(OperLogService::new(database.clone()));
    let file = Arc::new(FileService::new(database.clone(), object_storage.clone()));
    file.spawn_upload_janitor();
    let job_queue =
        Arc::new(JobQueue::new(database.clone()).with_wakeup_redis(redis_client.clone()));
    let tenant_data_migration = Arc::new(TenantDataMigrationService::new(
        database.clone(),
        tenant_data.clone(),
        job_queue.clone(),
        authorization_cache.clone(),
    ));
    let data_retention = Arc::new(DataRetentionService::new(
        database.clone(),
        job_queue.clone(),
        file.clone(),
        policies.retention,
    ));
    let user_import = Arc::new(UserImportService::new(
        database.clone(),
        job_queue.clone(),
        user.clone(),
        file.clone(),
        policies.user_import,
    ));
    let tenant_config_transfer = Arc::new(TenantConfigTransferService::new(
        ryframe_application::system::TenantConfigTransferDependencies {
            db: database.clone(),
            queue: job_queue.clone(),
            user_service: user.clone(),
            file_service: file.clone(),
            product_service: product.clone(),
            authorization_cache: authorization_cache.clone(),
        },
        ryframe_application::system::TenantConfigTransferSettings {
            target_catalog: ryframe_api::tenant_config_target_catalog()?,
            config: policies.tenant_config_transfer,
        },
    ));
    let authorization_diagnostic = Arc::new(AuthorizationDiagnosticService::new(
        database.clone(),
        user.clone(),
        authorization_cache.clone(),
        policies.messaging.enabled() && redis_client.is_some(),
    ));
    let overview = Arc::new(OverviewService::new(
        database.clone(),
        job_queue.clone(),
        policies.job_runtime,
    ));
    let job_schedules = if policies.job_schedule.enabled {
        let schedule_targets = super::jobs::build_schedule_targets(policies.messaging.enabled())?;
        Some(Arc::new(
            JobScheduleService::new(
                database.clone(),
                job_queue.clone(),
                super::jobs::execution_tenant_scope(policies.multi_tenancy),
                schedule_targets,
                policies.job_schedule,
            )
            .with_metrics_observer(super::jobs::build_schedule_metrics_observer()),
        ))
    } else {
        None
    };
    let audit_outbox = Arc::new(
        AuditOutbox::new(database.clone(), config.jobs.default_max_attempts)
            .with_job_queue(job_queue.clone()),
    );
    let message = Arc::new(MessageService::new(
        database.clone(),
        job_queue.clone(),
        policies.messaging,
    ));
    let websocket_ticket = Arc::new(WebSocketTicketService::new(
        redis_client.as_ref().map(|client| {
            Arc::new(RedisWebSocketTicketStore {
                client: client.clone(),
            }) as Arc<dyn WebSocketTicketStore>
        }),
        policies.messaging,
    ));
    let login_info = Arc::new(LoginInfoService::new(database.clone()));

    let profile = Arc::new(ProfileService::new(database.clone(), authorization_cache));
    let export = Arc::new(
        ExportService::new(
            database.clone(),
            user.clone(),
            object_storage,
            policies.export,
        )
        .with_job_queue(job_queue.clone()),
    );

    let refresh_sessions = auth.refresh_sessions();
    let online_user: Arc<OnlineUserService> = if let Some(redis) = redis_client {
        Arc::new(OnlineUserService::new_redis(
            redis.clone(),
            refresh_sessions,
        ))
    } else {
        Arc::new(OnlineUserService::new_in_memory(refresh_sessions))
    };
    let captcha: Arc<dyn CaptchaStore> = if let Some(redis) = redis_client {
        Arc::new(RedisCaptchaStore {
            client: redis.clone(),
            ttl_secs: 300,
        })
    } else {
        let store = InMemoryCaptchaStore::new(300);
        store.spawn_gc();
        Arc::new(store)
    };

    Ok(AppServices {
        auth,
        user,
        role,
        tenant,
        product,
        tenant_data: Arc::new(TenantRuntimeReader {
            router: tenant_data,
        }),
        tenant_usage,
        service_accounts,
        agent,
        permission,
        menu,
        dept,
        post,
        config: config_service,
        dict,
        export,
        notice,
        message,
        websocket_ticket,
        oper_log,
        audit_outbox,
        job_queue,
        job_schedules,
        data_retention,
        user_import,
        tenant_config_transfer,
        tenant_data_migration,
        authorization_diagnostic,
        overview,
        login_info,
        profile,
        file,
        online_user,
        captcha,
    })
}
