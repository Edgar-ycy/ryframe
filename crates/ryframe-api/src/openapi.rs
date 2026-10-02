use utoipa::{Modify, OpenApi};

mod domains;
mod extensions;
mod metadata;
mod modifier;
mod operations;
pub(crate) mod output;

use modifier::ApiDocModifier;

pub use output::render_openapi_json;
pub type OpenApiDocument = utoipa::openapi::OpenApi;

/// RyFrame API 文档。
pub struct ApiDoc;

/// 构建框架自身的 OpenAPI 文档。
pub fn document() -> OpenApiDocument {
    ApiDoc::openapi()
}

impl OpenApi for ApiDoc {
    fn openapi() -> utoipa::openapi::OpenApi {
        let mut document = metadata::document();
        domains::merge(&mut document);
        ApiDocModifier.modify(&mut document);
        document
    }
}

/// 获取 OpenAPI JSON 文档。
pub async fn openapi_json() -> impl axum::response::IntoResponse {
    output::openapi_json().await
}
