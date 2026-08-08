use ryframe_config::CorsConfig;
use ryframe_kernel::{AppError, AppResult};
use tower_http::cors::{AllowOrigin, CorsLayer};

/// 创建 CORS 层
///
/// 通过配置文件 `[cors]` section 中的 `allow_origins` 配置允许的源。
/// 空白来源列表拒绝跨域请求；生产环境必须显式配置管理端 Origin。
///
/// 示例：
/// ```toml
/// [cors]
/// allow_origins = ["http://localhost:80", "http://localhost:3000"]
/// ```
pub fn cors_layer(config: &CorsConfig) -> AppResult<CorsLayer> {
    let allow_origin = if config.allow_origins.is_empty() {
        tracing::info!("CORS: allow_origins 为空，拒绝所有跨域来源");
        None
    } else {
        tracing::info!("CORS: 允许来源 {:?}", config.allow_origins);
        let origins = config
            .allow_origins
            .iter()
            .map(|origin| {
                origin.parse().map_err(|error| {
                    AppError::Config(format!("无效的 CORS 来源 {origin:?}: {error}"))
                })
            })
            .collect::<AppResult<Vec<_>>>()?;
        Some(AllowOrigin::list(origins))
    };

    let mut layer = CorsLayer::new()
        .allow_methods([
            http::Method::GET,
            http::Method::POST,
            http::Method::PUT,
            http::Method::PATCH,
            http::Method::DELETE,
            http::Method::OPTIONS,
        ])
        .allow_headers([
            http::header::AUTHORIZATION,
            http::header::CONTENT_TYPE,
            http::header::ACCEPT,
            http::header::ORIGIN,
            http::header::ACCESS_CONTROL_REQUEST_METHOD,
            http::header::ACCESS_CONTROL_REQUEST_HEADERS,
            http::HeaderName::from_static("x-tenant-id"),
            http::HeaderName::from_static("x-csrf-token"),
            http::HeaderName::from_static("idempotency-key"),
            http::HeaderName::from_static("x-request-id"),
        ])
        .expose_headers([
            http::header::CONTENT_DISPOSITION,
            http::header::RETRY_AFTER,
            http::HeaderName::from_static("x-request-id"),
            http::HeaderName::from_static("x-idempotency-replay"),
            http::HeaderName::from_static("x-authorization-epoch"),
        ])
        .allow_credentials(true)
        .max_age(std::time::Duration::from_secs(3600));
    if let Some(allow_origin) = allow_origin {
        layer = layer.allow_origin(allow_origin);
    }
    Ok(layer)
}
