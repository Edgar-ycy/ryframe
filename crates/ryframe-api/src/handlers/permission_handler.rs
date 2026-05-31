use axum::{Json, Router, extract::State, routing::get};
use ryframe_common::{ApiResponse, AppResult};
use ryframe_service::system::PermissionTreeNode;

use super::auth_handler::AppState;

pub fn permission_router(state: AppState) -> Router {
    Router::new().route("/tree", get(tree)).with_state(state)
}

/// 权限树查询
#[utoipa::path(get, path = "/api/v1/system/permissions/tree", tag = "角色管理",
    responses((status = 200, description = "权限树")), security(("bearer" = [])))]
async fn tree(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<Vec<PermissionTreeNode>>>> {
    let tree = state.permission_service.find_tree(&state.db, None).await?;
    Ok(Json(ApiResponse::success(tree)))
}
