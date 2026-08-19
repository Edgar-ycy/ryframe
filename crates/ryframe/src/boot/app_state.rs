use std::sync::Arc;

use ryframe_api::{AppServices, runtime::RuntimeComponents};
use ryframe_config::{AppConfig, RedisMode};
use ryframe_core::{RedisClient, TokenBlacklist};
use ryframe_db::ControlDatabaseCluster;
use ryframe_i18n::Localizer;
use ryframe_middleware::RateLimiter;
use ryframe_utils::ip::TrustedProxySet;

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
    pub server_info: ryframe_monitor::ServerInfoSampler,
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
    let messaging = config.messaging.clone();
    let auth = ryframe_auth::middleware::AuthState {
        config: config.clone(),
        blacklist: token_blacklist.clone(),
        refresh_sessions: services.auth.refresh_sessions(),
        principal_resolver,
    };
    let monitor = ryframe_monitor::MonitorState {
        database: Arc::new(ryframe_db::SeaOrmDatabaseMonitor::new(database)),
        redis: redis_client.clone(),
        redis_configured: config
            .redis
            .as_ref()
            .is_some_and(|redis| redis.mode != RedisMode::Disabled),
        readiness: ryframe_monitor::DependencyHealthCache::new(
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
        config,
        localizer: localizer.clone(),
        services: Arc::new(services),
        redis: redis_client.clone(),
        message_hub: Arc::new(ryframe_api::message_socket::MessageHub::new(
            localizer, messaging,
        )),
        token_blacklist,
        rate_limiter: limiter,
        trusted_proxies,
        runtime: RuntimeComponents::new(redis_client),
    }
}
