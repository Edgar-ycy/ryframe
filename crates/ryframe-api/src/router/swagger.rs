use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::Path,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get as get_route,
};
use ryframe_http::{API_PREFIX, api_path};
use utoipa_swagger_ui::{Config as SwaggerUiConfig, serve as serve_swagger_ui};

const SWAGGER_UI_NO_CACHE: &str = "no-store";
const SWAGGER_UI_STATIC_CACHE: &str = "public, max-age=86400";

fn swagger_ui_base_element() -> String {
    format!("<base href=\"{}/swagger-ui/\">", API_PREFIX)
}

pub(super) fn swagger_ui_router() -> Router {
    Router::new()
        .route("/swagger-ui", get_route(swagger_ui_index))
        .route("/swagger-ui/{*asset}", get_route(swagger_ui_asset))
}

/// 返回唯一的 Swagger UI 文档入口，不提供尾斜杠兼容路由或重定向。
async fn swagger_ui_index() -> Response {
    swagger_ui_response("")
}

/// 返回编译进二进制的 Swagger UI 静态资源。
async fn swagger_ui_asset(Path(asset): Path<String>) -> Response {
    let asset = asset.trim_start_matches('/');
    if asset.is_empty() || asset == "index.html" || asset.contains('/') || asset.contains("..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    swagger_ui_response(asset)
}

fn swagger_ui_response(asset: &str) -> Response {
    let config = Arc::new(
        SwaggerUiConfig::from(api_path("api-docs/openapi.json"))
            .deep_linking(true)
            .default_models_expand_depth(1)
            .default_model_expand_depth(1)
            .doc_expansion("list")
            .filter(true)
            .show_extensions(true)
            .show_common_extensions(true)
            .validator_url("none"),
    );

    match serve_swagger_ui(asset, config) {
        Ok(Some(file)) => {
            let bytes = if asset.is_empty() {
                match localize_swagger_index(file.bytes.into_owned()) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        tracing::error!(%error, "无法解析内嵌 Swagger UI 首页");
                        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    }
                }
            } else {
                file.bytes.into_owned()
            };
            let cache_control = if asset.is_empty() || asset == "swagger-initializer.js" {
                SWAGGER_UI_NO_CACHE
            } else {
                SWAGGER_UI_STATIC_CACHE
            };

            match Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, file.content_type)
                .header(header::CACHE_CONTROL, cache_control)
                .body(Body::from(bytes))
            {
                Ok(response) => response,
                Err(error) => {
                    tracing::error!(%error, "无法构造内嵌 Swagger UI 响应");
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::error!(%error, "无法读取内嵌 Swagger UI 资源");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn localize_swagger_index(bytes: Vec<u8>) -> Result<Vec<u8>, std::string::FromUtf8Error> {
    let html = String::from_utf8(bytes)?
        .replacen("<html lang=\"en\">", "<html lang=\"zh-CN\">", 1)
        .replacen(
            "<head>",
            &format!("<head>\n    {}", swagger_ui_base_element()),
            1,
        )
        .replacen(
            "<title>Swagger UI</title>",
            "<title>RyFrame API 文档</title>",
            1,
        );
    Ok(html.into_bytes())
}
