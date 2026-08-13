use std::sync::Arc;

use ryframe_api::AppServices;
use ryframe_config::{AppConfig, RedisMode};
use ryframe_core::RedisClient;
use ryframe_db::DatabaseCluster;
use ryframe_kernel::AppError;
use ryframe_middleware::RateLimiter;
use ryframe_service::{
    AuditOutbox, AuthService, AuthorizationCache, JobQueue, JobScheduleService,
    agent::{AgentService, service_capability_descriptors},
    system::{
        AuthorizationDiagnosticService, CaptchaStore, ConfigService, DataRetentionService,
        DeptService, DictService, ExportService, FileService, GeneratorService, LoginInfoService,
        MenuService, MessageService, NoticeService, OnlineUserService, OperLogService,
        OverviewService, PermissionService, PostService, ProfileService, RoleService,
        ServiceAccountService, TenantConfigTransferService, TenantService, TenantUsageService,
        UserImportService, UserService, WebSocketTicketService,
    },
};
use ryframe_storage::ObjectStorage;

/// 构造所有 Service 实例
///
/// 依赖注入顺序：Repository → Redis → Service。
pub async fn build_all(
    database: &DatabaseCluster,
    config: &AppConfig,
    redis_client: &Option<RedisClient>,
    object_storage: Arc<dyn ObjectStorage>,
    rate_limiter: Arc<RateLimiter>,
) -> Result<AppServices, AppError> {
    let authorization_cache = AuthorizationCache::new(
        redis_client.clone(),
        config
            .redis
            .as_ref()
            .map(|redis| redis.mode)
            .unwrap_or(RedisMode::Disabled),
    );
    let user = Arc::new(UserService::new(
        database.clone(),
        authorization_cache.clone(),
    ));
    let role = Arc::new(RoleService::new(
        database.clone(),
        authorization_cache.clone(),
    ));
    let tenant = Arc::new(TenantService::new(
        database.clone(),
        authorization_cache.clone(),
    ));
    let tenant_usage = Arc::new(TenantUsageService::new(
        database.clone(),
        rate_limiter,
        config.rate_limit.enabled,
        config.jobs.scheduler_enabled,
    ));
    let (service_accounts, agent) = if config.service_accounts.enabled {
        let redis = redis_client.clone().ok_or_else(|| {
            AppError::Config("启用服务账号后必须配置 Redis，以保证 Agent 多实例限流一致".into())
        })?;
        let keyring = Arc::new(
            config
                .service_accounts
                .load_pepper_keyring(&config.auth.jwt_secret)
                .map_err(AppError::Config)?,
        );
        let descriptors = service_capability_descriptors();
        let management = Arc::new(ServiceAccountService::new(
            database.clone(),
            config.service_accounts.clone(),
            keyring.clone(),
            descriptors,
            authorization_cache.clone(),
        )?);
        let agent = Arc::new(AgentService::new(
            database.clone(),
            redis,
            keyring,
            config.service_accounts.clone(),
        )?);
        (Some(management), Some(agent))
    } else {
        (None, None)
    };
    let permission = Arc::new(PermissionService::new(
        database.clone(),
        authorization_cache.clone(),
    ));
    let auth = Arc::new(AuthService::new(
        database.clone(),
        Arc::new(config.clone()),
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

    let dict = Arc::new(DictService::new(database.clone(), redis_client.clone()));
    let notice = Arc::new(NoticeService::new(database.clone()));
    let oper_log = Arc::new(OperLogService::new(database.clone()));
    let file = Arc::new(FileService::new(database.clone(), object_storage.clone()));
    file.spawn_upload_janitor();
    let job_queue =
        Arc::new(JobQueue::new(database.clone()).with_wakeup_redis(redis_client.clone()));
    let data_retention = Arc::new(DataRetentionService::new(
        database.clone(),
        job_queue.clone(),
        file.clone(),
        config.data_retention.clone(),
        config.tenant_config_transfer.clone(),
    ));
    let user_import = Arc::new(UserImportService::new(
        database.clone(),
        job_queue.clone(),
        user.clone(),
        file.clone(),
        config.user_import.clone(),
    ));
    let tenant_config_transfer = Arc::new(TenantConfigTransferService::new(
        database.clone(),
        job_queue.clone(),
        user.clone(),
        file.clone(),
        authorization_cache.clone(),
        ryframe_api::tenant_config_target_catalog()?,
        config.tenant_config_transfer.clone(),
    ));
    let authorization_diagnostic = Arc::new(AuthorizationDiagnosticService::new(
        database.clone(),
        user.clone(),
        authorization_cache.clone(),
        config.messaging.enabled && redis_client.is_some(),
    ));
    let overview = Arc::new(OverviewService::new(
        database.clone(),
        job_queue.clone(),
        &config.jobs,
    ));
    let job_schedules = if config.jobs.scheduler_enabled {
        let schedule_targets = super::jobs::build_schedule_targets(config.messaging.enabled)?;
        Some(Arc::new(
            JobScheduleService::new(
                database.clone(),
                job_queue.clone(),
                schedule_targets,
                &config.jobs,
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
        config.messaging.clone(),
    ));
    let websocket_ticket = Arc::new(WebSocketTicketService::new(
        redis_client.clone(),
        config.messaging.clone(),
    ));
    let login_info = Arc::new(LoginInfoService::new(database.clone()));

    let project_root = std::env::current_dir()
        .map_err(|e| AppError::Internal(format!("无法获取项目根目录: {}", e)))?;
    if config.generator.data_source != "primary" {
        database
            .source(&config.generator.data_source)
            .ok_or_else(|| {
                AppError::Config(format!(
                    "代码生成器数据源未连接: {}",
                    config.generator.data_source
                ))
            })?;
    }
    let generator = Arc::new(GeneratorService::new(
        database.clone(),
        config.generator.data_source.clone(),
        project_root,
    ));

    let profile = Arc::new(ProfileService::new(
        database.clone(),
        authorization_cache.clone(),
    ));
    let export = Arc::new(
        ExportService::new(database.clone(), user.clone(), object_storage, &config.jobs)
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
    let captcha = if let Some(redis) = redis_client {
        CaptchaStore::new_redis(redis.clone(), 300)
    } else {
        let store = CaptchaStore::new_in_memory(300);
        store.spawn_gc(); // 内存模式需要后台 GC
        store
    };

    Ok(AppServices {
        auth,
        user,
        role,
        tenant,
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
        authorization_diagnostic,
        overview,
        login_info,
        generator,
        profile,
        file,
        online_user,
        captcha,
    })
}
