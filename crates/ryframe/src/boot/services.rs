use std::sync::Arc;

use ryframe_adapters::{RedisClient, rate_limit::RateLimiter};
use ryframe_api::AppServices;
use ryframe_application::{
    ArtifactStore, AuditOutbox, AuthService, JobQueue, JobScheduleService,
    agent::{AgentService, AgentServiceDependencies, service_capability_descriptors},
    system::{
        AuthorizationDiagnosticService, CaptchaStore, CaptchaStoreFuture, ConfigService,
        DataRetentionService, DeptService, DictCacheStore, DictCacheStoreFuture, DictService,
        ExportPersistencePorts, ExportResourceServices, ExportService, FileService,
        InMemoryCaptchaStore, LoginInfoService, MenuService, MessageService, NoticeService,
        OnlineUserService, OperLogService, OverviewService, PermissionService, PostService,
        ProductService, ProfileService, RoleService, ServiceAccountReadDependencies,
        ServiceAccountService, TenantConfigTransferService, TenantDataMigrationService,
        TenantRateLimitReadFuture, TenantRateLimitReadPort, TenantRateLimitSnapshot, TenantService,
        TenantUsageService, UserImportService, UserService, WebSocketTicketService,
        WebSocketTicketStore, WebSocketTicketStoreFuture,
    },
};
use ryframe_config::AppConfig;
use ryframe_db::ControlDatabaseCluster;
use ryframe_kernel::{AppError, CAPTCHA_KEY_PREFIX};
use ryframe_tenant_db::TenantDatabaseRouter;

use super::application_policy::{ApplicationPolicies, load_pepper_keyring};

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
    let authorization_cache =
        super::authorization_cache::cache(redis_client.clone(), policies.cache);
    let identity_read = ryframe_db::application_ports::auth::identity(database.clone());
    let user = Arc::new(UserService::new(
        authorization_cache.clone(),
        Arc::clone(&identity_read),
        ryframe_db::application_ports::user_query_persistence(database.clone()),
        ryframe_db::application_ports::user_write_persistence(
            database.clone(),
            authorization_cache.clone(),
        ),
        ryframe_db::application_ports::auth::password_reset(
            database.clone(),
            authorization_cache.clone(),
        ),
    ));
    let product = Arc::new(ProductService::new(
        ryframe_db::application_ports::product_read(database.clone()),
        ryframe_db::application_ports::product_write(database.clone()),
        authorization_cache.clone(),
        policies.service_accounts.enabled() && redis_client.is_some(),
    ));
    let role = Arc::new(RoleService::new(
        authorization_cache.clone(),
        ryframe_db::application_ports::role_read(database.clone()),
        ryframe_db::application_ports::role_write(
            database.clone(),
            authorization_cache.clone(),
            product.clone(),
        ),
    ));
    let tenant = Arc::new(TenantService::new(
        ryframe_tenant_db::tenant_persistence_port(database.clone()),
        authorization_cache.clone(),
        Arc::clone(&product),
        Arc::<TenantDatabaseRouter>::clone(&tenant_data),
    ));
    let tenant_usage = Arc::new(TenantUsageService::new(
        ryframe_db::application_ports::tenant_usage_persistence(database.clone()),
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
            ryframe_db::application_ports::service_accounts::write(database.clone()),
            policies.service_accounts,
            Arc::clone(&keyring),
            descriptors,
            authorization_cache.clone(),
            ServiceAccountReadDependencies {
                accounts: ryframe_db::application_ports::service_accounts::read(database.clone()),
                authorization: ryframe_db::application_ports::service_accounts::authorization(
                    database.clone(),
                ),
                audits: ryframe_db::application_ports::service_accounts::audit(database.clone()),
            },
        )?);
        let agent = Arc::new(AgentService::new(
            super::agent_limiter::redis_limiter(redis),
            keyring,
            policies.service_accounts,
            policies.multi_tenancy,
            AgentServiceDependencies {
                identity: ryframe_db::application_ports::agent_identity_read(database.clone()),
                audit: ryframe_db::application_ports::agent_audit_write(database.clone()),
                persistence: ryframe_db::application_ports::agent_persistence(
                    database.clone(),
                    Arc::clone(&product),
                ),
            },
        )?);
        (Some(management), Some(agent))
    } else {
        (None, None)
    };
    let permission = Arc::new(PermissionService::new(
        ryframe_db::application_ports::permission_read(database.clone()),
        ryframe_db::application_ports::permission_write(
            database.clone(),
            authorization_cache.clone(),
            Arc::clone(&product),
        ),
        authorization_cache.clone(),
    ));
    let token_settings = Arc::new(ryframe_auth::jwt::TokenSettings::new(
        Arc::<str>::from(config.auth.jwt_secret.as_str()),
        &config.auth.access_token_expire,
        &config.auth.refresh_token_expire,
    )?);
    let refresh_session_port = super::refresh_sessions::store(redis_client.clone());
    let auth = Arc::new(AuthService::new(
        identity_read,
        policies.auth,
        token_settings,
        super::login_protection::store(redis_client.clone()),
        Arc::clone(&refresh_session_port),
        authorization_cache.clone(),
    ));
    let menu = Arc::new(MenuService::new(
        ryframe_db::application_ports::menu_read(database.clone()),
        ryframe_db::application_ports::menu_write(database.clone(), authorization_cache.clone()),
        authorization_cache.clone(),
    ));

    let dept = Arc::new(DeptService::new(
        ryframe_db::application_ports::dept_read(database.clone()),
        ryframe_db::application_ports::dept_write(database.clone(), authorization_cache.clone()),
        authorization_cache.clone(),
    ));
    let post = Arc::new(PostService::new(
        ryframe_db::application_ports::post_persistence(database.clone()),
    ));
    let config_service = Arc::new(ConfigService::new(
        ryframe_db::application_ports::config_persistence(
            database.clone(),
            authorization_cache.clone(),
        ),
        authorization_cache.clone(),
    ));

    let dict = Arc::new(DictService::new(
        ryframe_db::application_ports::dict_persistence(database.clone()),
        redis_client.as_ref().map(|client| {
            Arc::new(RedisDictCacheStore {
                client: client.clone(),
            }) as Arc<dyn DictCacheStore>
        }),
    ));
    let notice = Arc::new(NoticeService::new(
        ryframe_db::application_ports::notice_persistence(database.clone()),
    ));
    let oper_log = Arc::new(OperLogService::new(
        ryframe_db::application_ports::oper_log_persistence(database.clone()),
    ));
    let file = Arc::new(FileService::new(
        ryframe_db::application_ports::files::cleanup(database.clone()),
        ryframe_db::application_ports::files::download(database.clone()),
        ryframe_db::application_ports::files::upload(database.clone()),
        object_storage.clone(),
        super::file_content::processor(),
    ));
    file.spawn_upload_janitor();
    let job_queue = Arc::new(
        JobQueue::new(ryframe_db::application_ports::jobs::queue(database.clone()))
            .with_wakeup_transport(super::jobs::job_wakeup_transport(redis_client.as_ref())),
    );
    let tenant_data_migration = Arc::new(TenantDataMigrationService::new(
        ryframe_tenant_db::tenant_data_migration_persistence_port(database.clone()),
        Arc::<TenantDatabaseRouter>::clone(&tenant_data),
        Arc::<TenantDatabaseRouter>::clone(&tenant_data),
        job_queue.clone(),
        authorization_cache.clone(),
    ));
    let data_retention = Arc::new(DataRetentionService::new(
        ryframe_db::application_ports::tenant_config_retention_persistence(database.clone()),
        ryframe_db::application_ports::retention_cleanup_persistence(database.clone()),
        ryframe_db::application_ports::retention_run_persistence(database.clone()),
        job_queue.clone(),
        file.clone(),
        policies.retention,
    ));
    let user_import = Arc::new(UserImportService::new(
        job_queue.clone(),
        user.clone(),
        file.clone(),
        super::spreadsheet::document_processor(),
        ryframe_db::application_ports::user_import_persistence(database.clone()),
        policies.user_import,
    ));
    let tenant_config_transfer = Arc::new(TenantConfigTransferService::new(
        ryframe_application::system::TenantConfigTransferDependencies {
            persistence: ryframe_db::application_ports::tenant_config_transfer_persistence(
                database.clone(),
            ),
            queue: job_queue.clone(),
            user_service: user.clone(),
            file_service: file.clone(),
            product_service: product.clone(),
            authorization_cache: authorization_cache.clone(),
            archive: super::tenant_config_archive::codec(),
        },
        ryframe_application::system::TenantConfigTransferSettings {
            target_catalog: ryframe_api::tenant_config_target_catalog()?,
            config: policies.tenant_config_transfer,
        },
    ));
    let authorization_diagnostic = Arc::new(AuthorizationDiagnosticService::new(
        ryframe_db::application_ports::authorization::diagnostic(database.clone()),
        user.clone(),
        authorization_cache.clone(),
        policies.messaging.enabled() && redis_client.is_some(),
    ));
    let overview = Arc::new(OverviewService::new(
        ryframe_db::application_ports::overview_persistence(database.clone()),
        job_queue.clone(),
        policies.job_runtime,
    ));
    let job_schedules = if policies.job_schedule.enabled {
        let schedule_targets = super::jobs::build_schedule_targets(policies.messaging.enabled())?;
        Some(Arc::new(
            JobScheduleService::new(
                ryframe_db::application_ports::jobs::schedule(database.clone()),
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
        AuditOutbox::new(
            ryframe_db::application_ports::audit_outbox_persistence(database.clone()),
            config.jobs.default_max_attempts,
        )
        .with_job_queue(job_queue.clone()),
    );
    let message = Arc::new(MessageService::new(
        ryframe_db::application_ports::message_persistence(database.clone()),
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
    let login_info = Arc::new(LoginInfoService::new(
        ryframe_db::application_ports::login_info_persistence(database.clone()),
    ));

    let profile = Arc::new(ProfileService::new(
        ryframe_db::application_ports::profile_persistence(
            database.clone(),
            authorization_cache.clone(),
        ),
        authorization_cache,
    ));
    let export = Arc::new(
        ExportService::new(
            ExportPersistencePorts::new(
                ryframe_db::application_ports::export::artifact(database.clone()),
                ryframe_db::application_ports::export::cleanup(database.clone()),
                ryframe_db::application_ports::export::deletion(database.clone()),
                ryframe_db::application_ports::export::execution(database.clone()),
                ryframe_db::application_ports::export::request(database.clone()),
                ryframe_db::application_ports::export::requester(database.clone()),
            ),
            ExportResourceServices {
                users: Arc::clone(&user),
                roles: Arc::clone(&role),
                posts: Arc::clone(&post),
                configs: Arc::clone(&config_service),
                dicts: Arc::clone(&dict),
                oper_logs: Arc::clone(&oper_log),
                login_infos: Arc::clone(&login_info),
            },
            object_storage,
            super::spreadsheet::writer_factory(),
            policies.export,
        )
        .with_job_queue(job_queue.clone()),
    );

    let online_user: Arc<OnlineUserService> = if let Some(redis) = redis_client {
        Arc::new(OnlineUserService::new(
            super::online_sessions::redis_store(redis.clone()),
            refresh_session_port,
        ))
    } else {
        Arc::new(OnlineUserService::new_in_memory(refresh_session_port))
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
        tenant_data,
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
