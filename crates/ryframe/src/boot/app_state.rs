use std::sync::Arc;

use ryframe_adapters::{RedisClient, TokenBlacklist, rate_limit::RateLimiter};
use ryframe_api::{
    AppServices, HttpRuntimeSettings, TrustedProxySet,
    runtime::RuntimeComponents,
    settings::{
        CorsSettings, JobRuntimeSettings, MessagingSettings, MultiTenancySettings,
        RateLimitSettings, StorageRuntimeSettings, UploadSettings,
    },
};
use ryframe_config::{AppConfig, JobWorkerMode, RedisMode, StorageBackend};
use ryframe_db::ControlDatabaseCluster;
use ryframe_kernel::Localizer;

/// 组装 API 状态前已经就绪的依赖。
///
/// 使用单个输入结构保持组合根的依赖关系显式，避免随着基础设施增加让函数参数失控。
pub struct AppStateAssembly {
    pub database: ControlDatabaseCluster,
    pub config: Arc<AppConfig>,
    pub localizer: Arc<Localizer>,
    pub redis_client: Option<RedisClient>,
    pub token_blacklist: TokenBlacklist,
    pub services: AppServices,
    pub limiter: Arc<RateLimiter>,
    pub server_info: ryframe_adapters::monitor::ServerInfoSampler,
}

fn http_runtime_settings(config: &AppConfig) -> HttpRuntimeSettings {
    HttpRuntimeSettings {
        production: config.environment.is_production(),
        telemetry_enabled: config.telemetry.enabled,
        api_docs_enabled: config.api_docs.enabled,
        pagination: (&config.pagination).into(),
        multi_tenancy: MultiTenancySettings {
            enabled: config.multi_tenancy.enabled,
        },
        upload: UploadSettings {
            file_max_bytes: config.upload.file_max_bytes,
            avatar_max_bytes: config.upload.avatar_max_bytes,
            multipart_envelope_bytes: config.upload.multipart_envelope_bytes,
            upload_timeout_seconds: config.upload.upload_timeout_seconds,
            api_timeout_seconds: config.upload.api_timeout_seconds,
        },
        cors: CorsSettings {
            allow_origins: config.cors.allow_origins.clone(),
        },
        rate_limit: rate_limit_settings(config),
        messaging: MessagingSettings {
            enabled: config.messaging.enabled,
            max_connections_per_user: config.messaging.max_connections_per_user,
            outbound_buffer: config.messaging.outbound_buffer,
            replay_interval_seconds: config.messaging.replay_interval_seconds,
            replay_jitter_seconds: config.messaging.replay_jitter_seconds,
            replay_batch_size: config.messaging.replay_batch_size,
        },
        jobs: JobRuntimeSettings {
            mode: match config.jobs.mode {
                JobWorkerMode::Embedded => "embedded",
                JobWorkerMode::External => "external",
                JobWorkerMode::Disabled => "disabled",
            }
            .into(),
            scheduler_enabled: config.jobs.scheduler_enabled,
        },
        object_storage: StorageRuntimeSettings {
            backend: config.object_storage.backend.as_str().into(),
            endpoint: (config.object_storage.backend != StorageBackend::Local
                && !config.object_storage.endpoint.trim().is_empty())
            .then(|| config.object_storage.endpoint.clone()),
        },
        redis_configured: config
            .redis
            .as_ref()
            .is_some_and(|redis| redis.mode != RedisMode::Disabled),
        user_import_max_file_bytes: config.user_import.max_file_bytes,
        tenant_config_max_package_bytes: config.tenant_config_transfer.max_package_bytes,
    }
}

pub(super) fn rate_limit_settings(config: &AppConfig) -> RateLimitSettings {
    RateLimitSettings {
        enabled: config.rate_limit.enabled,
        capacity: config.rate_limit.capacity,
        window_secs: config.rate_limit.window_secs,
        enable_user_rate_limit: config.rate_limit.enable_user_rate_limit,
        user_window_secs: config.rate_limit.user_window_secs,
        user_capacity: config.rate_limit.user_capacity,
        api_limits: config.rate_limit.api_limits.clone(),
        api_window_secs: config.rate_limit.api_window_secs,
    }
}

/// 将所有已初始化的组件聚合为 AppState。
pub fn assemble(assembly: AppStateAssembly) -> ryframe_api::AppState {
    let AppStateAssembly {
        database,
        config,
        localizer,
        redis_client,
        token_blacklist,
        services,
        limiter,
        server_info,
    } = assembly;
    let trusted_proxies = TrustedProxySet::new(&config.proxy.trusted_cidrs)
        .expect("proxy CIDRs were validated during configuration loading");
    let principal_resolver = services.auth.clone();
    let settings = Arc::new(http_runtime_settings(&config));
    let auth = ryframe_api::auth_middleware::AuthState {
        token_settings: services.auth.token_settings(),
        allow_multiple_tenants: config.multi_tenancy.enabled,
        blacklist: token_blacklist.clone(),
        refresh_sessions: services.auth.refresh_sessions(),
        principal_resolver,
    };
    let monitor = ryframe_api::monitor::MonitorState {
        database: super::readiness::database_monitor(database),
        redis: redis_client.clone(),
        redis_configured: config
            .redis
            .as_ref()
            .is_some_and(|redis| redis.mode != RedisMode::Disabled),
        readiness: ryframe_adapters::monitor::DependencyHealthCache::new(
            config
                .redis
                .as_ref()
                .is_some_and(|redis| redis.mode == RedisMode::Required),
            true,
            super::readiness::CACHE_MAX_AGE,
        ),
        metrics_bearer_token: Arc::from(config.monitor.metrics_bearer_token.as_str()),
        server_info,
    };

    ryframe_api::AppState {
        auth,
        monitor,
        settings: settings.clone(),
        localizer: localizer.clone(),
        services: Arc::new(services),
        redis: redis_client.clone(),
        message_hub: Arc::new(ryframe_api::message_socket::MessageHub::new(
            localizer,
            settings.messaging.clone(),
        )),
        token_blacklist,
        rate_limiter: super::limiter::http_limiter(limiter),
        trusted_proxies,
        runtime: RuntimeComponents::new(redis_client),
    }
}
