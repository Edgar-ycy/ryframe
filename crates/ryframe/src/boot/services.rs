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
    generated::{GeneratedPersistencePorts, GeneratedServices},
    ports::{auth::RefreshSessionPort, files::ArtifactStore},
    system::{
        identity::{
            CaptchaStore, DeptService, InMemoryCaptchaStore, MenuService, PermissionService,
            ProfileService, WebSocketTicketService,
        },
        operations::OnlineUserService,
        platform::{AuthorizationDiagnosticService, TenantService, TenantUsageService},
    },
};
use ryframe_config::AppConfig;
use ryframe_db::ControlDatabaseCluster;
use ryframe_kernel::AppError;
use ryframe_tenant_db::TenantDatabaseRouter;

use super::application_policy::ApplicationPolicies;
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
    let refresh_sessions = super::refresh_sessions::store(redis_client.clone());
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
        Arc::clone(&refresh_sessions),
        &background,
    )?;
    let content = build_content_services(generated, &background);
    let operations = build_operations_services(
        database,
        config,
        redis_client,
        refresh_sessions,
        &background,
    );

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
    let authorization_diagnostic = Arc::new(AuthorizationDiagnosticService::new(
        ryframe_db::application_ports::authorization::diagnostic(database.clone()),
        Arc::clone(&background.user),
        background.authorization_cache.clone(),
        policies.messaging.enabled() && redis_client.is_some(),
    ));

    Ok(PlatformServices {
        tenant,
        product: Arc::clone(&background.product),
        tenant_data,
        tenant_usage,
        tenant_config_transfer: Arc::clone(&background.tenant_config_transfer),
        tenant_data_migration: Arc::clone(&background.tenant_data_migration),
        authorization_diagnostic,
    })
}

fn build_identity_services(
    database: &ControlDatabaseCluster,
    config: &AppConfig,
    policies: &ApplicationPolicies,
    redis_client: &Option<RedisClient>,
    starts_background_tasks: bool,
    refresh_sessions: Arc<dyn RefreshSessionPort>,
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
    let auth = Arc::new(AuthService::new(
        Arc::clone(&background.identity_read),
        policies.auth,
        token_settings,
        super::login_protection::store(redis_client.clone()),
        refresh_sessions,
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
    let profile = Arc::new(ProfileService::new(
        ryframe_db::application_ports::users::profile(
            database.clone(),
            background.authorization_cache.clone(),
        ),
        background.authorization_cache.clone(),
    ));
    Ok(IdentityServices {
        auth,
        user: Arc::clone(&background.user),
        role: Arc::clone(&background.role),
        permission,
        menu,
        dept,
        user_import: Arc::clone(&background.user_import),
        profile,
        captcha: build_captcha_store(redis_client, starts_background_tasks),
        websocket_ticket: build_websocket_ticket_service(redis_client, policies),
    })
}

fn build_captcha_store(
    redis_client: &Option<RedisClient>,
    starts_background_tasks: bool,
) -> Arc<dyn CaptchaStore> {
    if let Some(redis) = redis_client {
        return redis_captcha_store(redis.clone(), 300);
    }
    let store = InMemoryCaptchaStore::new(300);
    if starts_background_tasks {
        store.spawn_gc();
    }
    Arc::new(store)
}

fn build_websocket_ticket_service(
    redis_client: &Option<RedisClient>,
    policies: &ApplicationPolicies,
) -> Arc<WebSocketTicketService> {
    Arc::new(WebSocketTicketService::new(
        redis_client
            .as_ref()
            .map(|client| redis_websocket_ticket_store(client.clone())),
        policies.messaging,
    ))
}

fn build_content_services(
    generated: GeneratedServices,
    background: &BackgroundServices,
) -> ContentServices {
    ContentServices {
        generated,
        config: Arc::clone(&background.config),
        dict: Arc::clone(&background.dict),
        file: Arc::clone(&background.file),
    }
}

fn build_operations_services(
    database: &ControlDatabaseCluster,
    config: &AppConfig,
    redis_client: &Option<RedisClient>,
    refresh_sessions: Arc<dyn RefreshSessionPort>,
    background: &BackgroundServices,
) -> OperationsServices {
    let audit_outbox = Arc::new(
        AuditOutbox::new(
            ryframe_db::application_ports::audit::outbox(database.clone()),
            config.jobs.default_max_attempts,
        )
        .with_job_queue(Arc::clone(&background.job_queue)),
    );
    let online_user: Arc<OnlineUserService> = if let Some(redis) = redis_client {
        Arc::new(OnlineUserService::new(
            super::online_sessions::redis_store(redis.clone()),
            refresh_sessions,
        ))
    } else {
        Arc::new(OnlineUserService::new_in_memory(refresh_sessions))
    };
    OperationsServices {
        message: Arc::clone(&background.message),
        online_user,
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
