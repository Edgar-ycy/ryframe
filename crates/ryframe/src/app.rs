use axum::{
    Router,
    middleware::{from_fn, from_fn_with_state},
    routing::get,
};
use ryframe_api::middleware as http_middleware;
use ryframe_api::middleware::{
    rate_limit::RateLimitState, security_headers::SecurityHeadersConfig,
};
use ryframe_api::request_locale::request_locale_middleware;
use ryframe_config::CorsConfig;
use ryframe_kernel::AppResult;

/// 将公开探针与业务路由分开构建，确保存活/就绪检查绝不会经过认证、租户提取、
/// 幂等控制或业务限流。
pub fn build_app(
    state: ryframe_api::AppState,
    rate_limit_state: RateLimitState,
    cors_config: &CorsConfig,
) -> AppResult<Router> {
    ryframe_api::validate_runtime_features(&state.config)?;
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
    let agent_security_headers = security_headers.clone();

    let business = ryframe_api::VersionedRouter::new()
        .with_v1(ryframe_api::api_router(
            state.clone(),
            rate_limit_state_for_api,
        ))
        .into_router()
        .layer(from_fn_with_state(
            upload_limits.clone(),
            http_middleware::body_limit::body_limit_middleware,
        ))
        .layer(from_fn_with_state(
            upload_limits,
            http_middleware::timeout::timeout_middleware,
        ))
        .layer(from_fn_with_state(
            security_headers,
            http_middleware::security_headers::security_headers_middleware,
        ))
        .layer(from_fn_with_state(
            rate_limit_state.clone(),
            http_middleware::rate_limit::api_rate_limit_middleware,
        ))
        .layer(from_fn_with_state(
            rate_limit_state,
            http_middleware::rate_limit::rate_limit_middleware,
        ))
        .layer(from_fn(request_locale_middleware));

    let probes = Router::new()
        .route("/livez", get(ryframe_api::livez))
        .route("/readyz", get(ryframe_api::readyz))
        .with_state(state.clone());

    let regular = Router::new()
        .merge(business)
        .merge(probes)
        .layer(http_middleware::cors::cors_layer(cors_config)?)
        .layer(from_fn_with_state(
            response_localizer.clone(),
            http_middleware::response_envelope::api_response_envelope_middleware,
        ))
        .layer(http_middleware::compression_layer());

    // Agent API 不经过会在业务审计前短路的通用请求体、超时、限流或 CORS 层。
    // 其固定 GET 路由在服务内执行配置限定的总预算、专用原子限流和 fail-closed 审计；
    // OPTIONS 与未知方法也会进入 Agent fallback 并留下最小审计。
    let agent = if state.services.agent.is_some() {
        Router::new()
            .nest(
                "/api/v1/agent/v1",
                ryframe_api::handlers::agent_handler::agent_router(state),
            )
            .layer(from_fn_with_state(
                agent_security_headers,
                http_middleware::security_headers::security_headers_middleware,
            ))
            .layer(from_fn(request_locale_middleware))
            .layer(from_fn_with_state(
                response_localizer,
                http_middleware::response_envelope::api_response_envelope_middleware,
            ))
    } else {
        Router::new()
    };

    let app = Router::new()
        .merge(regular)
        .merge(agent)
        .layer(http_middleware::request_log::request_log_layer_with_masking())
        .layer(from_fn_with_state(
            trusted_proxies,
            http_middleware::client_ip::trusted_client_ip_middleware,
        ))
        .layer(from_fn(http_middleware::request_id::request_id_middleware))
        .layer(from_fn(http_middleware::metrics::metrics_middleware));

    if telemetry_enabled {
        Ok(app.layer(from_fn(http_middleware::telemetry::telemetry_middleware)))
    } else {
        Ok(app)
    }
}
