use utoipa::OpenApi;

use super::ApiDoc;

/// 以确定性的对象键排序渲染 OpenAPI 文档。
///
/// Utoipa 将扩展字段存入哈希映射，直接序列化 `OpenApi` 可能使原本相同的进程
/// 产生字节级差异。
pub fn render_openapi_json(
    document: &utoipa::openapi::OpenApi,
) -> Result<String, serde_json::Error> {
    let canonical = serde_json::to_value(document)?;
    Ok(format!("{}\n", serde_json::to_string_pretty(&canonical)?))
}

pub(super) async fn openapi_json() -> impl axum::response::IntoResponse {
    use axum::Json;

    Json(ApiDoc::openapi())
}
