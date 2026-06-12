use std::sync::Arc;

use ryframe_config::AppConfig;
use ryframe_core::RedisClient;
use ryframe_middleware::{RateLimitState, RateLimiter};

/// 限流器初始化结果
pub struct LimiterState {
    pub limiter: Arc<RateLimiter>,
    pub rate_limit_state: RateLimitState,
}

/// 初始化限流器（Redis 固定窗口 / 内存令牌桶 双模式）
pub fn init(config: &AppConfig, redis_client: &Option<RedisClient>) -> LimiterState {
    let limiter = if let Some(redis) = redis_client {
        let window = if config.rate_limit.window_secs > 0 {
            config.rate_limit.window_secs
        } else {
            60 // 默认 60 秒固定窗口
        };
        Arc::new(RateLimiter::new_redis(
            redis.clone(),
            config.rate_limit.capacity,
            window,
        ))
    } else {
        let l = Arc::new(RateLimiter::new_in_memory(
            config.rate_limit.capacity,
            config.rate_limit.refill_per_sec,
        ));
        l.spawn_gc();
        l
    };

    let rate_limit_state = RateLimitState {
        limiter: limiter.clone(),
        config: Arc::new(config.rate_limit.clone()),
    };

    LimiterState {
        limiter,
        rate_limit_state,
    }
}
