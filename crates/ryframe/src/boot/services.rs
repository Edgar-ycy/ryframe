use std::sync::Arc;

use async_trait::async_trait;
use ryframe_adapters::{RedisClient, rate_limit::RateLimiter};
use ryframe_api::{
    AppServices, ContentServices, IdentityServices, OperationsServices, PlatformServices,
};
use ryframe_application::{
    AuditOutbox, AuthService,
    agent::{AgentService, AgentServiceDependencies, service_capability_descriptors},
    generated::{GeneratedPersistencePorts, GeneratedServices},
    ports::files::ArtifactStore,
    system::{
        AuthorizationDiagnosticService, CaptchaStore, DeptService, DictCacheStore,
        InMemoryCaptchaStore, MenuService, NoticeService, OnlineUserService, PermissionService,
        ProfileService, ServiceAccountReadDependencies, ServiceAccountService,
        TenantRateLimitReadPort, TenantRateLimitSnapshot, TenantService, TenantUsageService,
        WebSocketTicketService, WebSocketTicketStore,
    },
};
use ryframe_config::AppConfig;
use ryframe_db::ControlDatabaseCluster;
use ryframe_kernel::{AppError, CAPTCHA_KEY_PREFIX};
use ryframe_tenant_db::TenantDatabaseRouter;

use super::application_policy::{ApplicationPolicies, load_pepper_keyring};
use super::background_services::{
    BackgroundServiceInfrastructure, BackgroundServices, build as build_background_services,
};

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

#[async_trait]
impl DictCacheStore for RedisDictCacheStore {
    async fn get(&self, key: &str) -> Result<Option<String>, AppError> {
        self.client
            .get(key)
            .await
            .map_err(|error| AppError::ServiceUnavailable(error.to_string()))
    }

    async fn put(&self, key: String, value: String, ttl_secs: u64) -> Result<(), AppError> {
        self.client
            .set_ex(key, value, ttl_secs)
            .await
            .map_err(|error| AppError::ServiceUnavailable(error.to_string()))
    }

    async fn remove(&self, key: String) -> Result<(), AppError> {
        self.client
            .del(key)
            .await
            .map(|_| ())
            .map_err(|error| AppError::ServiceUnavailable(error.to_string()))
    }
}

#[async_trait]
impl CaptchaStore for RedisCaptchaStore {
    async fn set(&self, id: String, answer: String) -> Result<(), AppError> {
        let key = format!("{CAPTCHA_KEY_PREFIX}{id}");
        self.client
            .set_ex(key, answer, self.ttl_secs)
            .await
            .map_err(|error| {
                tracing::error!(%error, "Redis SET 验证码失败");
                AppError::ServiceUnavailable("验证码服务暂不可用".into())
            })
    }

    async fn verify(&self, id: &str, code: &str) -> Result<bool, AppError> {
        let key = format!("{CAPTCHA_KEY_PREFIX}{id}");
        self.client
            .get_and_del(&key)
            .await
            .map(|stored| stored.is_some_and(|value| value.eq_ignore_ascii_case(code)))
            .map_err(|error| {
                tracing::error!(%error, "Redis GETDEL 验证码失败");
                AppError::ServiceUnavailable("验证码服务暂不可用".into())
            })
    }
}

#[async_trait]
impl WebSocketTicketStore for RedisWebSocketTicketStore {
    async fn put(&self, key: String, value: String, ttl_secs: u64) -> Result<(), AppError> {
        self.client
            .set_ex(key, value, ttl_secs)
            .await
            .map_err(|error| {
                AppError::ServiceUnavailable(format!("WebSocket 票据写入失败: {error}"))
            })
    }

    async fn take(&self, key: &str) -> Result<Option<String>, AppError> {
        self.client.get_and_del(key).await.map_err(|error| {
            AppError::ServiceUnavailable(format!("WebSocket 票据校验失败: {error}"))
        })
    }
}

#[async_trait]
impl TenantRateLimitReadPort for TenantRateLimitReader {
    async fn snapshot_many(
        &self,
        tenant_ids: &[String],
    ) -> Result<Vec<TenantRateLimitSnapshot>, AppError> {
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
    }
}

/// 构造所有 Service 实例
///
/// 依赖注入顺序：Repository → Redis → Service。
pub struct ServiceInfrastructure<'a> {
    pub redis_client: &'a Option<RedisClient>,
    pub object_storage: Arc<dyn ArtifactStore>,
    pub rate_limiter: Arc<RateLimiter>,
    pub starts_background_tasks: bool,
}

pub async fn build_all(
    database: &ControlDatabaseCluster,
    tenant_data: Arc<TenantDatabaseRouter>,
    config: &AppConfig,
    policies: &ApplicationPolicies,
    infrastructure: ServiceInfrastructure<'_>,
) -> Result<AppServices, AppError> {
    let ServiceInfrastructure {
        redis_client,
        object_storage,
        rate_limiter,
        starts_background_tasks,
    } = infrastructure;
    let mut generated_ports = GeneratedPersistencePorts::default();
    ryframe_db::generated::register_ports(database.clone(), &mut generated_ports);
    ryframe_tenant_db::generated::register_ports(
        Arc::<TenantDatabaseRouter>::clone(&tenant_data),
        &mut generated_ports,
    );
    let generated = GeneratedServices::try_new(generated_ports)?;
    let background = build_background_services(
        database,
        Arc::clone(&tenant_data),
        policies,
        BackgroundServiceInfrastructure {
            redis_client: redis_client.clone(),
            object_storage,
            dict_cache: redis_client.as_ref().map(|client| {
                Arc::new(RedisDictCacheStore {
                    client: client.clone(),
                }) as Arc<dyn DictCacheStore>
            }),
            starts_background_tasks,
        },
    )?;
    let BackgroundServices {
        authorization_cache,
        identity_read,
        user,
        product,
        role,
        config: config_service,
        dict,
        oper_log,
        login_info,
        file,
        job_queue,
        job_schedules,
        message,
        export,
        data_retention,
        user_import,
        tenant_config_transfer,
        tenant_data_migration,
        overview,
    } = background;
    let tenant = Arc::new(TenantService::new(
        ryframe_tenant_db::application_ports::tenants::registry(database.clone()),
        authorization_cache.clone(),
        Arc::clone(&product),
        Arc::<TenantDatabaseRouter>::clone(&tenant_data),
    ));
    let tenant_usage = Arc::new(TenantUsageService::new(
        ryframe_db::application_ports::tenants::usage(database.clone()),
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
                identity: ryframe_db::application_ports::agent::identity(database.clone()),
                audit: ryframe_db::application_ports::agent::audit(database.clone()),
                persistence: ryframe_db::application_ports::agent::storage(
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
        ryframe_db::application_ports::system::permission_read(database.clone()),
        ryframe_db::application_ports::system::permission_write(
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
        ryframe_db::application_ports::system::menu_read(database.clone()),
        ryframe_db::application_ports::system::menu_write(
            database.clone(),
            authorization_cache.clone(),
        ),
        authorization_cache.clone(),
    ));

    let dept = Arc::new(DeptService::new(
        ryframe_db::application_ports::system::dept_read(database.clone()),
        ryframe_db::application_ports::system::dept_write(
            database.clone(),
            authorization_cache.clone(),
        ),
        authorization_cache.clone(),
    ));
    let notice = Arc::new(NoticeService::new(
        ryframe_db::application_ports::system::notice(database.clone()),
    ));
    let authorization_diagnostic = Arc::new(AuthorizationDiagnosticService::new(
        ryframe_db::application_ports::authorization::diagnostic(database.clone()),
        user.clone(),
        authorization_cache.clone(),
        policies.messaging.enabled() && redis_client.is_some(),
    ));
    let audit_outbox = Arc::new(
        AuditOutbox::new(
            ryframe_db::application_ports::audit::outbox(database.clone()),
            config.jobs.default_max_attempts,
        )
        .with_job_queue(job_queue.clone()),
    );
    let websocket_ticket = Arc::new(WebSocketTicketService::new(
        redis_client.as_ref().map(|client| {
            Arc::new(RedisWebSocketTicketStore {
                client: client.clone(),
            }) as Arc<dyn WebSocketTicketStore>
        }),
        policies.messaging,
    ));
    let profile = Arc::new(ProfileService::new(
        ryframe_db::application_ports::users::profile(
            database.clone(),
            authorization_cache.clone(),
        ),
        authorization_cache,
    ));
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
        if starts_background_tasks {
            store.spawn_gc();
        }
        Arc::new(store)
    };

    Ok(AppServices {
        identity: IdentityServices {
            auth,
            user,
            role,
            permission,
            menu,
            dept,
            user_import,
            authorization_diagnostic,
            profile,
            online_user,
            captcha,
        },
        platform: PlatformServices {
            tenant,
            product,
            tenant_data,
            tenant_usage,
            service_accounts,
            agent,
            tenant_config_transfer,
            tenant_data_migration,
        },
        content: ContentServices {
            generated,
            config: config_service,
            dict,
            notice,
            message,
            websocket_ticket,
            file,
        },
        operations: OperationsServices {
            export,
            oper_log,
            audit_outbox,
            job_queue,
            job_schedules,
            data_retention,
            overview,
            login_info,
        },
    })
}
