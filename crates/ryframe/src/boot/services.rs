use std::sync::Arc;

use ryframe_adapters::{
    RedisClient,
    application_ports::{
        redis_captcha_store, redis_dict_cache_store, redis_websocket_ticket_store,
        tenant_rate_limit_reader,
    },
    rate_limit::RateLimiter,
};
use ryframe_api::{
    AppServices, ContentServices, IdentityServices, OperationsServices, PlatformServices,
};
use ryframe_application::{
    AuditOutbox, AuthService,
    agent::{AgentService, AgentServiceDependencies, service_capability_descriptors},
    generated::{GeneratedPersistencePorts, GeneratedServices},
    ports::files::ArtifactStore,
    system::{
        AuthorizationDiagnosticService, CaptchaStore, DeptService, InMemoryCaptchaStore,
        MenuService, NoticeService, OnlineUserService, PermissionService, ProfileService,
        ServiceAccountReadDependencies, ServiceAccountService, TenantService, TenantUsageService,
        WebSocketTicketService,
    },
};
use ryframe_config::AppConfig;
use ryframe_db::ControlDatabaseCluster;
use ryframe_kernel::AppError;
use ryframe_tenant_db::TenantDatabaseRouter;

use super::application_policy::{ApplicationPolicies, load_pepper_keyring};
use super::background_services::{
    BackgroundServiceInfrastructure, BackgroundServices, build as build_background_services,
};

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
            dict_cache: redis_client
                .as_ref()
                .map(|client| redis_dict_cache_store(client.clone())),
            starts_background_tasks,
        },
    )?;
    let platform = build_platform_services(
        database,
        Arc::clone(&tenant_data),
        config,
        policies,
        redis_client,
        rate_limiter,
        &background,
    )?;
    let identity = build_identity_services(
        database,
        config,
        policies,
        redis_client,
        starts_background_tasks,
        &background,
    )?;
    let content = build_content_services(database, policies, redis_client, generated, &background);
    let operations = build_operations_services(database, config, &background);

    Ok(AppServices {
        identity,
        platform,
        content,
        operations,
    })
}

fn build_platform_services(
    database: &ControlDatabaseCluster,
    tenant_data: Arc<TenantDatabaseRouter>,
    config: &AppConfig,
    policies: &ApplicationPolicies,
    redis_client: &Option<RedisClient>,
    rate_limiter: Arc<RateLimiter>,
    background: &BackgroundServices,
) -> Result<PlatformServices, AppError> {
    let tenant = Arc::new(TenantService::new(
        ryframe_tenant_db::application_ports::tenants::registry(database.clone()),
        background.authorization_cache.clone(),
        Arc::clone(&background.product),
        Arc::<TenantDatabaseRouter>::clone(&tenant_data),
    ));
    let tenant_usage = Arc::new(TenantUsageService::new(
        ryframe_db::application_ports::tenants::usage(database.clone()),
        tenant_rate_limit_reader(rate_limiter),
        config.rate_limit.enabled,
        policies.job_schedule.enabled,
    ));
    let (service_accounts, agent) =
        build_service_account_services(database, config, policies, redis_client, background)?;

    Ok(PlatformServices {
        tenant,
        product: Arc::clone(&background.product),
        tenant_data,
        tenant_usage,
        service_accounts,
        agent,
        tenant_config_transfer: Arc::clone(&background.tenant_config_transfer),
        tenant_data_migration: Arc::clone(&background.tenant_data_migration),
    })
}

fn build_service_account_services(
    database: &ControlDatabaseCluster,
    config: &AppConfig,
    policies: &ApplicationPolicies,
    redis_client: &Option<RedisClient>,
    background: &BackgroundServices,
) -> Result<
    (
        Option<Arc<ServiceAccountService>>,
        Option<Arc<AgentService>>,
    ),
    AppError,
> {
    if !policies.service_accounts.enabled() {
        return Ok((None, None));
    }
    let redis = redis_client.clone().ok_or_else(|| {
        AppError::Config("启用服务账号后必须配置 Redis，以保证 Agent 多实例限流一致".into())
    })?;
    let keyring = load_pepper_keyring(config)?;
    let management = Arc::new(ServiceAccountService::new(
        ryframe_db::application_ports::service_accounts::write(database.clone()),
        policies.service_accounts,
        Arc::clone(&keyring),
        service_capability_descriptors(),
        background.authorization_cache.clone(),
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
                Arc::clone(&background.product),
            ),
        },
    )?);
    Ok((Some(management), Some(agent)))
}

fn build_identity_services(
    database: &ControlDatabaseCluster,
    config: &AppConfig,
    policies: &ApplicationPolicies,
    redis_client: &Option<RedisClient>,
    starts_background_tasks: bool,
    background: &BackgroundServices,
) -> Result<IdentityServices, AppError> {
    let permission = Arc::new(PermissionService::new(
        ryframe_db::application_ports::system::permission_read(database.clone()),
        ryframe_db::application_ports::system::permission_write(
            database.clone(),
            background.authorization_cache.clone(),
            Arc::clone(&background.product),
        ),
        background.authorization_cache.clone(),
    ));
    let token_settings = Arc::new(ryframe_auth::jwt::TokenSettings::new(
        Arc::<str>::from(config.auth.jwt_secret.as_str()),
        &config.auth.access_token_expire,
        &config.auth.refresh_token_expire,
    )?);
    let refresh_session_port = super::refresh_sessions::store(redis_client.clone());
    let auth = Arc::new(AuthService::new(
        Arc::clone(&background.identity_read),
        policies.auth,
        token_settings,
        super::login_protection::store(redis_client.clone()),
        Arc::clone(&refresh_session_port),
        background.authorization_cache.clone(),
    ));
    let menu = Arc::new(MenuService::new(
        ryframe_db::application_ports::system::menu_read(database.clone()),
        ryframe_db::application_ports::system::menu_write(
            database.clone(),
            background.authorization_cache.clone(),
        ),
        background.authorization_cache.clone(),
    ));

    let dept = Arc::new(DeptService::new(
        ryframe_db::application_ports::system::dept_read(database.clone()),
        ryframe_db::application_ports::system::dept_write(
            database.clone(),
            background.authorization_cache.clone(),
        ),
        background.authorization_cache.clone(),
    ));
    let authorization_diagnostic = Arc::new(AuthorizationDiagnosticService::new(
        ryframe_db::application_ports::authorization::diagnostic(database.clone()),
        Arc::clone(&background.user),
        background.authorization_cache.clone(),
        policies.messaging.enabled() && redis_client.is_some(),
    ));
    let profile = Arc::new(ProfileService::new(
        ryframe_db::application_ports::users::profile(
            database.clone(),
            background.authorization_cache.clone(),
        ),
        background.authorization_cache.clone(),
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
        redis_captcha_store(redis.clone(), 300)
    } else {
        let store = InMemoryCaptchaStore::new(300);
        if starts_background_tasks {
            store.spawn_gc();
        }
        Arc::new(store)
    };

    Ok(IdentityServices {
        auth,
        user: Arc::clone(&background.user),
        role: Arc::clone(&background.role),
        permission,
        menu,
        dept,
        user_import: Arc::clone(&background.user_import),
        authorization_diagnostic,
        profile,
        online_user,
        captcha,
    })
}

fn build_content_services(
    database: &ControlDatabaseCluster,
    policies: &ApplicationPolicies,
    redis_client: &Option<RedisClient>,
    generated: GeneratedServices,
    background: &BackgroundServices,
) -> ContentServices {
    let notice = Arc::new(NoticeService::new(
        ryframe_db::application_ports::system::notice(database.clone()),
    ));
    let websocket_ticket = Arc::new(WebSocketTicketService::new(
        redis_client
            .as_ref()
            .map(|client| redis_websocket_ticket_store(client.clone())),
        policies.messaging,
    ));
    ContentServices {
        generated,
        config: Arc::clone(&background.config),
        dict: Arc::clone(&background.dict),
        notice,
        message: Arc::clone(&background.message),
        websocket_ticket,
        file: Arc::clone(&background.file),
    }
}

fn build_operations_services(
    database: &ControlDatabaseCluster,
    config: &AppConfig,
    background: &BackgroundServices,
) -> OperationsServices {
    let audit_outbox = Arc::new(
        AuditOutbox::new(
            ryframe_db::application_ports::audit::outbox(database.clone()),
            config.jobs.default_max_attempts,
        )
        .with_job_queue(Arc::clone(&background.job_queue)),
    );
    OperationsServices {
        export: Arc::clone(&background.export),
        oper_log: Arc::clone(&background.oper_log),
        audit_outbox,
        job_queue: Arc::clone(&background.job_queue),
        job_schedules: background.job_schedules.clone(),
        data_retention: Arc::clone(&background.data_retention),
        overview: Arc::clone(&background.overview),
        login_info: Arc::clone(&background.login_info),
    }
}
