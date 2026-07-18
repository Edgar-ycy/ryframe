use ryframe_common::{AppError, AppResult};
use ryframe_config::{RedisConfig, RedisMode};
use ryframe_core::{RedisClient, TokenBlacklist};

pub struct RedisState {
    pub client: Option<RedisClient>,
    pub token_blacklist: TokenBlacklist,
}

async fn flush_config_cache(redis: &RedisClient) {
    const PREFIX: &str = "sys_config:key:";
    match redis.delete_by_pattern(&format!("{PREFIX}*")).await {
        Ok(deleted) if deleted > 0 => {
            tracing::info!(deleted, "cleared stale configuration cache entries");
        }
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, "failed to clear configuration cache"),
    }
}

pub async fn init(config: &Option<RedisConfig>) -> AppResult<RedisState> {
    let mode = config
        .as_ref()
        .map_or(RedisMode::Disabled, |config| config.mode);
    let client = match (mode, config.as_ref()) {
        (RedisMode::Disabled, _) | (_, None) => {
            tracing::warn!("Redis is explicitly disabled; using single-instance memory state");
            ryframe_middleware::metrics::record_redis_degraded("startup_disabled");
            None
        }
        (RedisMode::Required | RedisMode::Optional, Some(redis_config)) => {
            match connect_and_verify(redis_config).await {
                Ok(client) => Some(client),
                Err(error) if mode.is_required() => return Err(error),
                Err(error) => {
                    tracing::warn!(%error, "Redis optional mode is degraded to local memory");
                    ryframe_middleware::metrics::record_redis_degraded("startup_optional");
                    None
                }
            }
        }
    };

    if let Some(redis) = &client {
        if is_production() {
            verify_production_policy(redis).await?;
        }
        flush_config_cache(redis).await;
    }

    let token_blacklist = TokenBlacklist::new(client.clone());
    if client.is_none() {
        token_blacklist.spawn_gc();
    }
    Ok(RedisState {
        client,
        token_blacklist,
    })
}

async fn connect_and_verify(config: &RedisConfig) -> AppResult<RedisClient> {
    let client = RedisClient::connect(config).await.map_err(|error| {
        AppError::ServiceUnavailable(format!("Redis connection failed: {error}"))
    })?;
    client
        .ping()
        .await
        .map_err(|error| AppError::ServiceUnavailable(format!("Redis PING failed: {error}")))?;
    tracing::info!(mode = ?config.mode, host = %config.host, port = config.port, "Redis is ready");
    Ok(client)
}

async fn verify_production_policy(redis: &RedisClient) -> AppResult<()> {
    let eviction = redis
        .config_get("maxmemory-policy")
        .await
        .map_err(redis_policy_error)?
        .unwrap_or_default();
    if eviction != "noeviction" {
        return Err(AppError::Config(format!(
            "production Redis requires maxmemory-policy=noeviction (found {eviction:?})"
        )));
    }

    let append_only = redis
        .config_get("appendonly")
        .await
        .map_err(redis_policy_error)?
        .unwrap_or_default();
    let save_schedule = redis
        .config_get("save")
        .await
        .map_err(redis_policy_error)?
        .unwrap_or_default();
    if append_only != "yes" && save_schedule.trim().is_empty() {
        return Err(AppError::Config(
            "production Redis must enable AOF or snapshot persistence".into(),
        ));
    }
    Ok(())
}

fn redis_policy_error(error: redis::RedisError) -> AppError {
    AppError::Config(format!("unable to verify production Redis policy: {error}"))
}

fn is_production() -> bool {
    std::env::var("APP_ENV").is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "prod" | "production"
        )
    })
}
