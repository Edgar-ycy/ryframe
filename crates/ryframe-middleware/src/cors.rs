use tower_http::cors::{AllowOrigin, CorsLayer};
use ryframe_config::CorsConfig;

/// 创建 CORS 层
///
/// 通过配置文件 `[cors]` section 中的 `allow_origins` 配置允许的源。
/// 未配置时默认使用 mirror_request 模式（兼容 credentials）。
///
/// 示例：
/// ```toml
/// [cors]
/// allow_origins = ["http://localhost:5173", "http://localhost:3000"]
/// ```
pub fn cors_layer(config: &CorsConfig) -> CorsLayer {
    let allow_origin = if config.allow_origins.is_empty() {
        // 开发环境默认镜像请求源（兼容 credentials）
        tracing::info!("CORS: 未配置 allow_origins，使用 mirror_request 模式");
        AllowOrigin::mirror_request()
    } else {
        tracing::info!("CORS: 允许来源 {:?}", config.allow_origins);
        AllowOrigin::list(config.allow_origins.iter().map(|o| o.parse().unwrap()))
    };

    CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods([
            http::Method::GET,
            http::Method::POST,
            http::Method::PUT,
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
        ])
        .expose_headers([http::header::CONTENT_DISPOSITION])
        .allow_credentials(true)
        .max_age(std::time::Duration::from_secs(3600))
}
