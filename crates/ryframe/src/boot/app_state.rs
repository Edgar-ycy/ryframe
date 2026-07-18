use std::sync::Arc;

use ryframe_api::{AppServices, runtime::RuntimeComponents};
use ryframe_common::utils::ip::TrustedProxySet;
use ryframe_config::AppConfig;
use ryframe_core::{RedisClient, TokenBlacklist};
use ryframe_db::DatabaseCluster;
use ryframe_middleware::RateLimiter;

/// 将所有已初始化的组件聚合为 AppState
pub fn assemble(
    database: DatabaseCluster,
    config: Arc<AppConfig>,
    redis_client: Option<RedisClient>,
    token_blacklist: TokenBlacklist,
    services: AppServices,
    limiter: Arc<RateLimiter>,
) -> ryframe_api::AppState {
    let trusted_proxies = TrustedProxySet::new(&config.proxy.trusted_cidrs)
        .expect("proxy CIDRs were validated during configuration loading");
    let principal_resolver = services.auth.clone();
    let auth = ryframe_auth::middleware::AuthState {
        config: config.clone(),
        blacklist: token_blacklist.clone(),
        refresh_sessions: services.auth.refresh_sessions(),
        principal_resolver,
    };
    let monitor = ryframe_monitor::MonitorState {
        database: Arc::new(ryframe_db::SeaOrmDatabaseMonitor::new(database)),
        redis: redis_client.clone(),
    };

    ryframe_api::AppState {
        auth,
        monitor,
        config,
        services: Arc::new(services),
        redis: redis_client.clone(),
        token_blacklist,
        rate_limiter: limiter,
        trusted_proxies,
        runtime: RuntimeComponents::new(redis_client),
    }
}
