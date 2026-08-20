use std::sync::Arc;

use ryframe_adapters::{RedisClient, rate_limit::RateLimiter};
use ryframe_api::middleware::rate_limit::RateLimitState;
use ryframe_api::rate_limit::{HttpRateLimiter, RateLimitFuture};
use ryframe_config::AppConfig;
use ryframe_kernel::AppResult;

/// 限流器初始化结果
pub struct LimiterState {
    pub limiter: Arc<RateLimiter>,
    pub rate_limit_state: RateLimitState,
}

struct HttpRateLimiterBridge {
    limiter: Arc<RateLimiter>,
}

impl HttpRateLimiter for HttpRateLimiterBridge {
    fn acquire<'a>(&'a self, key: &'a str, window_secs: u64, limit: u32) -> RateLimitFuture<'a> {
        Box::pin(async move {
            self.limiter
                .acquire(key, window_secs, limit)
                .await
                .map(|decision| ryframe_api::rate_limit::RateLimitDecision {
                    allowed: decision.allowed,
                    retry_after_secs: decision.retry_after_secs,
                })
        })
    }
}

pub fn http_limiter(limiter: Arc<RateLimiter>) -> Arc<dyn HttpRateLimiter> {
    Arc::new(HttpRateLimiterBridge { limiter })
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
        limiter: http_limiter(Arc::clone(&limiter)),
        config: Arc::new(super::app_state::rate_limit_settings(config)),
    };

    Ok(LimiterState {
        limiter,
        rate_limit_state,
    })
}
