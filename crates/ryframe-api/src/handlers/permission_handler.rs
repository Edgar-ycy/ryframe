use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use ryframe_common::{ApiResponse, AppResult};
use ryframe_macro::{delete, get, post, put, route};
use ryframe_service::system::{PermissionSyncReport, PermissionTreeNode};
use validator::Validate;

use super::auth_handler::AppState;
use crate::dto::permission_dto::{CreatePermissionDto, UpdatePermissionDto};

#[derive(Debug, serde::Deserialize)]
pub struct PermissionListQuery {
    pub perm_type: Option<String>,
}

pub fn permission_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(tree))
        .merge(route!(detail))
        .merge(route!(create))
        .merge(route!(update))
        .merge(route!(remove))
        .merge(route!(sync_perm_from_route))
        .with_state(state)
}

#[get("/tree")]
#[perm("system:perm:list")]
#[utoipa::path(get, path = "/api/v1/system/perms/tree", tag = "权限管理",
    params(("perm_type" = Option<String>, Query)),
    responses((status = 200, description = "权限树")), security(("bearer" = [])))]
pub async fn tree(
    State(state): State<AppState>,
    Query(query): Query<PermissionListQuery>,
) -> AppResult<Json<ApiResponse<Vec<PermissionTreeNode>>>> {
    let tree = state
        .permission_service
        .list_all_perms(&state.db, query.perm_type.as_deref())
        .await?;
    Ok(Json(ApiResponse::success(tree)))
}

#[get("/{id}")]
#[perm("system:perm:list")]
#[utoipa::path(get, path = "/api/v1/system/perms/{id}", tag = "权限管理",
    params(("id" = i64, Path)), responses((status = 200, description = "权限详情")),
    security(("bearer" = [])))]
pub async fn detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<ryframe_db::entities::permission::Model>>> {
    let item = state.permission_service.find_by_id(&state.db, id).await?;
    match item {
        Some(item) => Ok(Json(ApiResponse::success(item))),
        None => Err(ryframe_common::AppError::NotFound("权限不存在".into())),
    }
}

#[post("/")]
#[perm("system:perm:add")]
#[utoipa::path(post, path = "/api/v1/system/perms", tag = "权限管理",
    request_body = CreatePermissionDto, responses((status = 200, description = "创建成功")),
    security(("bearer" = [])))]
pub async fn create(
    State(state): State<AppState>,
    Json(dto): Json<CreatePermissionDto>,
) -> AppResult<Json<ApiResponse<ryframe_db::entities::permission::Model>>> {
    dto.validate()?;
    let item = state
        .permission_service
        .create(
            &state.db,
            &dto.name,
            &dto.code,
            dto.parent_id,
            &dto.perm_type,
            dto.icon.as_deref(),
            dto.sort.unwrap_or(0),
            dto.status.as_deref().unwrap_or("1"),
        )
        .await?;
    Ok(Json(ApiResponse::success(item)))
}

#[put("/{id}")]
#[perm("system:perm:edit")]
#[utoipa::path(put, path = "/api/v1/system/perms/{id}", tag = "权限管理",
    params(("id" = i64, Path)), request_body = UpdatePermissionDto,
    responses((status = 200, description = "更新成功")), security(("bearer" = [])))]
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdatePermissionDto>,
) -> AppResult<Json<ApiResponse<ryframe_db::entities::permission::Model>>> {
    dto.validate()?;
    let item = state
        .permission_service
        .update(
            &state.db,
            id,
            &dto.name,
            &dto.code,
            dto.parent_id,
            &dto.perm_type,
            dto.icon.as_deref(),
            dto.sort.unwrap_or(0),
            dto.status.as_deref().unwrap_or("1"),
        )
        .await?;
    state.menu_service.invalidate_menu_cache();
    Ok(Json(ApiResponse::success(item)))
}

#[delete("/{id}")]
#[perm("system:perm:remove")]
#[utoipa::path(delete, path = "/api/v1/system/perms/{id}", tag = "权限管理",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功")),
    security(("bearer" = [])))]
pub async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    state.permission_service.delete(&state.db, id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("删除成功")))
}

#[post("/sync")]
#[perm("system:perm:sync")]
#[utoipa::path(post, path = "/api/v1/system/perms/sync", tag = "权限管理",
    responses((status = 200, description = "同步成功")), security(("bearer" = [])))]
pub async fn sync_perm_from_route(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<PermissionSyncReport>>> {
    let report = state
        .permission_service
        .sync_perm_from_route(&state.db)
        .await?;
    Ok(Json(ApiResponse::success(report)))
}
