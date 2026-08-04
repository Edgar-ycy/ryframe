use axum::{
    Router,
    middleware::{from_fn, from_fn_with_state},
    routing::get,
};
use ryframe_api::request_locale::request_locale_middleware;
use ryframe_config::CorsConfig;
use ryframe_kernel::AppResult;
use ryframe_middleware::{SecurityHeadersConfig, rate_limit::RateLimitState};

/// 将公开探针与业务路由分开构建，确保存活/就绪检查绝不会经过认证、租户提取、
/// 幂等控制或业务限流。
pub fn build_app(
    state: ryframe_api::AppState,
    rate_limit_state: RateLimitState,
    cors_config: &CorsConfig,
) -> AppResult<Router> {
    let trusted_proxies = state.trusted_proxies.clone();
    let response_localizer = state.localizer.clone();
    let upload_limits = state.config.upload.clone();
    let telemetry_enabled = state.config.telemetry.enabled;
    let rate_limit_state_for_api = rate_limit_state.clone();
    let security_headers = if state.config.environment.is_production() {
        SecurityHeadersConfig::strict()
    } else {
        SecurityHeadersConfig::default()
    };

    let business = ryframe_api::VersionedRouter::new()
        .with_v1(ryframe_api::api_router(
            state.clone(),
            rate_limit_state_for_api,
        ))
        .into_router()
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
        ))
        .layer(from_fn(request_locale_middleware));

    let probes = Router::new()
        .route("/livez", get(ryframe_api::livez))
        .route("/readyz", get(ryframe_api::readyz))
        .with_state(state);

    let app = Router::new()
        .merge(business)
        .merge(probes)
        .layer(ryframe_middleware::cors_layer(cors_config)?)
        .layer(from_fn_with_state(
            response_localizer,
            ryframe_middleware::api_response_envelope_middleware,
        ))
        .layer(ryframe_middleware::compression_layer())
        .layer(ryframe_middleware::request_log_layer_with_masking())
        .layer(from_fn_with_state(
            trusted_proxies,
            ryframe_middleware::trusted_client_ip_middleware,
        ))
        .layer(from_fn(ryframe_middleware::request_id_middleware))
        .layer(from_fn(ryframe_middleware::metrics::metrics_middleware));

    if telemetry_enabled {
        Ok(app.layer(from_fn(ryframe_middleware::telemetry::telemetry_middleware)))
    } else {
        Ok(app)
    }
}
