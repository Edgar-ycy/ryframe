use std::sync::Arc;

use ryframe_adapters::{
    RedisClient, TokenBlacklist,
    monitor::{self as runtime_monitor, ServerInfoSampler},
    rate_limit::RateLimiter,
    resilience::{CircuitBreaker, CircuitState},
};
use ryframe_api::{
    AppServices, HttpRuntimeSettings, TrustedProxySet,
    monitor::{
        CacheCommandStats, CacheCommandStatsFuture, CacheCommandStatsStatus, CacheInfo,
        CacheInfoFuture, CacheKeysInfo, CacheMonitor, DependencyHealthCache, RedisMemoryInfo,
        RedisServerInfo, ServerInfo, ServerInfoMonitor,
    },
    runtime::{RuntimeComponents, UploadCircuitBreaker},
    settings::{
        CorsSettings, JobRuntimeSettings, MessagingSettings, MultiTenancySettings,
        RateLimitSettings, StorageRuntimeSettings, UploadSettings,
    },
};
use ryframe_config::{AppConfig, JobWorkerMode, RedisMode, StorageBackend};
use ryframe_db::ControlDatabaseCluster;
use ryframe_kernel::Localizer;

struct UploadCircuitBreakerBridge {
    breaker: CircuitBreaker,
}

impl UploadCircuitBreaker for UploadCircuitBreakerBridge {
    fn allow_request(&self) -> bool {
        self.breaker.allow_request()
    }

    fn record_success(&self) {
        self.breaker.record_success();
    }

    fn record_failure(&self) {
        self.breaker.record_failure();
    }

    fn state_label(&self) -> &'static str {
        match self.breaker.current_state() {
            CircuitState::Closed => "Closed",
            CircuitState::Open => "Open",
            CircuitState::HalfOpen => "HalfOpen",
        }
    }
}

struct CacheMonitorBridge {
    redis: Option<RedisClient>,
    redis_configured: bool,
}

impl CacheMonitor for CacheMonitorBridge {
    fn info(&self) -> CacheInfoFuture<'_> {
        Box::pin(async move {
            map_cache_info(runtime_monitor::get_cache_info(self.redis.as_ref()).await)
        })
    }

    fn command_stats(&self) -> CacheCommandStatsFuture<'_> {
        Box::pin(async move {
            let stats = match self.redis.as_ref() {
                Some(redis) => runtime_monitor::get_cache_command_stats(redis).await,
                None if self.redis_configured => runtime_monitor::CacheCommandStats::unavailable(),
                None => runtime_monitor::CacheCommandStats::not_configured(),
            };
            CacheCommandStats {
                status: match stats.status {
                    runtime_monitor::CacheCommandStatsStatus::Available => {
                        CacheCommandStatsStatus::Available
                    }
                    runtime_monitor::CacheCommandStatsStatus::NotConfigured => {
                        CacheCommandStatsStatus::NotConfigured
                    }
                    runtime_monitor::CacheCommandStatsStatus::Unavailable => {
                        CacheCommandStatsStatus::Unavailable
                    }
                },
                commands: stats.commands,
            }
        })
    }
}

fn map_cache_info(value: runtime_monitor::CacheInfo) -> CacheInfo {
    CacheInfo {
        available: value.available,
        mode: value.mode,
        server: value.server.map(|server| RedisServerInfo {
            version: server.version,
            mode: server.mode,
            os: server.os,
            uptime_days: server.uptime_days,
            connected_clients: server.connected_clients,
        }),
        keys: CacheKeysInfo {
            total_keys: value.keys.total_keys,
            online_users: value.keys.online_users,
            captchas: value.keys.captchas,
            rate_limits: value.keys.rate_limits,
            dict_cache: value.keys.dict_cache,
            config_cache: value.keys.config_cache,
        },
        memory: value.memory.map(|memory| RedisMemoryInfo {
            used_memory_human: memory.used_memory_human,
            used_memory_peak_human: memory.used_memory_peak_human,
            mem_fragmentation_ratio: memory.mem_fragmentation_ratio,
            used_memory: memory.used_memory,
        }),
    }
}

struct ServerInfoMonitorBridge {
    sampler: ServerInfoSampler,
}

impl ServerInfoMonitor for ServerInfoMonitorBridge {
    fn latest(&self) -> ServerInfo {
        let value = self.sampler.latest();
        ServerInfo {
            os: value.os.to_string(),
            hostname: value.hostname.to_string(),
            cpu_cores: value.cpu_cores,
            cpu_usage: value.cpu_usage,
            total_memory: value.total_memory,
            used_memory: value.used_memory,
            memory_usage: value.memory_usage,
            pid: value.pid,
            uptime: value.uptime,
        }
    }
}

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
    pub server_info: ServerInfoSampler,
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
    let access_revocations = super::session_security::access_revocations(token_blacklist);
    let refresh_sessions =
        super::session_security::refresh_sessions(services.auth.refresh_sessions());
    let auth = ryframe_api::auth_middleware::AuthState {
        token_settings: services.auth.token_settings(),
        allow_multiple_tenants: config.multi_tenancy.enabled,
        access_revocations,
        refresh_sessions,
        principal_resolver,
    };
    let redis_configured = config
        .redis
        .as_ref()
        .is_some_and(|redis| redis.mode != RedisMode::Disabled);
    let monitor = ryframe_api::monitor::MonitorState {
        database: super::readiness::database_monitor(database),
        cache: Arc::new(CacheMonitorBridge {
            redis: redis_client.clone(),
            redis_configured,
        }),
        readiness: DependencyHealthCache::new(
            config
                .redis
                .as_ref()
                .is_some_and(|redis| redis.mode == RedisMode::Required),
            true,
            super::readiness::CACHE_MAX_AGE,
        ),
        metrics_bearer_token: Arc::from(config.monitor.metrics_bearer_token.as_str()),
        server_info: Arc::new(ServerInfoMonitorBridge {
            sampler: server_info,
        }),
    };
    let idempotency_store = super::idempotency::store(redis_client.clone());
    let redis_connected = redis_client.is_some();

    ryframe_api::AppState {
        auth,
        monitor,
        settings: settings.clone(),
        localizer: localizer.clone(),
        services: Arc::new(services),
        redis_connected,
        idempotency_store,
        message_hub: Arc::new(ryframe_api::message_socket::MessageHub::new(
            localizer,
            settings.messaging.clone(),
        )),
        rate_limiter: super::limiter::http_limiter(limiter),
        trusted_proxies,
        runtime: RuntimeComponents::new(Arc::new(UploadCircuitBreakerBridge {
            breaker: CircuitBreaker::default_config(),
        })),
    }
}
