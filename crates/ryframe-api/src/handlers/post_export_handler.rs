use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use ryframe_macro::{post, route};

use crate::{
    RequestPrincipal,
    dto::{export_dto::PostExportRequestDto, public_dto::ExportJobVo},
    handlers::export_handler::request_export,
    http::{ApiResponse, HttpResult},
    state::AppState,
};

pub fn post_export_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(request_post_export))
        .with_state(state)
}

/// 创建岗位异步导出任务。
#[post("/exports")]
#[perm("system:post:export")]
#[utoipa::path(post, path = "/api/v1/system/posts/exports", tag = "岗位管理",
    params(("Idempotency-Key" = String, Header, description = "幂等键")), request_body = PostExportRequestDto,
    responses((status = 202, description = "岗位导出任务已创建", body = ApiResponse<ExportJobVo>)), security(("bearer" = [])))]
pub async fn request_post_export(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    headers: HeaderMap,
    Json(request): Json<PostExportRequestDto>,
) -> HttpResult<(StatusCode, Json<ApiResponse<ExportJobVo>>)> {
    let (selection, confirm_all) = request.into_selection();
    request_export(
        state,
        current_user,
        headers,
        "system:post:export",
        selection,
        confirm_all,
    )
    .await
}
