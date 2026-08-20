use std::sync::Arc;

use ryframe_adapters::{RedisClient, rate_limit::RateLimiter};
use ryframe_api::middleware::rate_limit::RateLimitState;
use ryframe_config::AppConfig;
use ryframe_kernel::AppResult;

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
        config: Arc::new(super::app_state::rate_limit_settings(config)),
    };

    Ok(LimiterState {
        limiter,
        rate_limit_state,
    })
}
