use axum::{
    Router,
    middleware::{from_fn, from_fn_with_state},
    routing::get,
};
use ryframe_common::AppResult;
use ryframe_config::CorsConfig;
use ryframe_middleware::{SecurityHeadersConfig, rate_limit::RateLimitState};

/// Build public probes separately from business routes so liveness/readiness
/// never pass through authentication, tenant extraction, idempotency, or
/// business rate limiting.
pub fn build_app(
    state: ryframe_api::AppState,
    rate_limit_state: RateLimitState,
    cors_config: &CorsConfig,
) -> AppResult<Router> {
    let trusted_proxies = state.trusted_proxies.clone();
    let upload_limits = state.config.upload.clone();
    let rate_limit_state_for_api = rate_limit_state.clone();
    let security_headers = if is_production() {
        SecurityHeadersConfig::strict()
    } else {
        SecurityHeadersConfig::default()
    };

    let business = Router::new()
        .nest(
            "/api/v1",
            ryframe_api::api_router(state.clone(), rate_limit_state_for_api),
        )
        .layer(from_fn(ryframe_middleware::xss_filter))
        .layer(from_fn_with_state(
            upload_limits.clone(),
            ryframe_middleware::body_limit_middleware,
        ))
        .layer(from_fn_with_state(
            upload_limits,
            ryframe_middleware::timeout_middleware,
        ))
        .layer(from_fn_with_state(
            security_headers,
            ryframe_middleware::security_headers_middleware,
        ))
        .layer(from_fn_with_state(
            rate_limit_state.clone(),
            ryframe_middleware::api_rate_limit_middleware,
        ))
        .layer(from_fn_with_state(
            rate_limit_state,
            ryframe_middleware::rate_limit_middleware,
        ));

    let probes = Router::new()
        .route("/livez", get(ryframe_api::livez))
        .route("/readyz", get(ryframe_api::readyz))
        .with_state(state);

    Ok(Router::new()
        .merge(business)
        .merge(probes)
        .layer(ryframe_middleware::cors_layer(cors_config)?)
        .layer(ryframe_middleware::compression_layer())
        .layer(ryframe_middleware::request_log_layer_with_masking())
        .layer(from_fn_with_state(
            trusted_proxies,
            ryframe_middleware::trusted_client_ip_middleware,
        ))
        .layer(from_fn(ryframe_middleware::request_id_middleware))
        .layer(from_fn(ryframe_middleware::metrics::metrics_middleware)))
}

fn is_production() -> bool {
    std::env::var("APP_ENV").is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "prod" | "production"
        )
    })
}
