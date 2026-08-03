use std::sync::Arc;

use ryframe_config::AppConfig;
use ryframe_core::RedisClient;
use ryframe_kernel::{AppError, AppResult};
use ryframe_middleware::{RateLimitState, RateLimiter};
use ryframe_utils::ip::TrustedProxySet;

/// 限流器初始化结果
pub struct LimiterState {
    pub limiter: Arc<RateLimiter>,
    pub rate_limit_state: RateLimitState,
}

/// 初始化固定窗口限流器。
pub fn init(config: &AppConfig, redis_client: &Option<RedisClient>) -> AppResult<LimiterState> {
    let limiter = if let Some(redis) = redis_client {
        Arc::new(RateLimiter::new_redis(
            redis.clone(),
            config.rate_limit.capacity,
            config.rate_limit.window_secs,
        ))
    } else {
        let l = Arc::new(RateLimiter::new_in_memory(
            config.rate_limit.capacity,
            config.rate_limit.window_secs,
        ));
        l.spawn_gc();
        l
    };

    let rate_limit_state = RateLimitState {
        limiter: limiter.clone(),
        config: Arc::new(config.rate_limit.clone()),
        trusted_proxies: TrustedProxySet::new(&config.proxy.trusted_cidrs)
            .map_err(AppError::Config)?,
    };

    Ok(LimiterState {
        limiter,
        rate_limit_state,
    })
}
