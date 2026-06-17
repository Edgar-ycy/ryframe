use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, post, put},
};
use ryframe_auth::middleware::perm_route;
use ryframe_common::{ApiResponse, AppResult};
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
        .route("/tree", perm_route(get(tree), "system:permission:list"))
        .route("/", perm_route(post(create), "system:permission:add"))
        .route("/{id}", perm_route(get(detail), "system:permission:list"))
        .route("/{id}", perm_route(put(update), "system:permission:edit"))
        .route(
            "/{id}",
            perm_route(delete(remove), "system:permission:remove"),
        )
        .route(
            "/sync",
            perm_route(post(sync_api_permissions), "system:permission:sync"),
        )
        .with_state(state)
}

#[utoipa::path(get, path = "/api/v1/system/permissions/tree", tag = "权限管理",
    params(("perm_type" = Option<String>, Query)),
    responses((status = 200, description = "权限树")), security(("bearer" = [])))]
pub async fn tree(
    State(state): State<AppState>,
    Query(query): Query<PermissionListQuery>,
) -> AppResult<Json<ApiResponse<Vec<PermissionTreeNode>>>> {
    let tree = state
        .permission_service
        .find_tree(&state.db, query.perm_type.as_deref())
        .await?;
    Ok(Json(ApiResponse::success(tree)))
}

#[utoipa::path(get, path = "/api/v1/system/permissions/{id}", tag = "权限管理",
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

#[utoipa::path(post, path = "/api/v1/system/permissions", tag = "权限管理",
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

#[utoipa::path(put, path = "/api/v1/system/permissions/{id}", tag = "权限管理",
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
    Ok(Json(ApiResponse::success(item)))
}

#[utoipa::path(delete, path = "/api/v1/system/permissions/{id}", tag = "权限管理",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功")),
    security(("bearer" = [])))]
pub async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    state.permission_service.delete(&state.db, id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("删除成功")))
}

#[utoipa::path(post, path = "/api/v1/system/permissions/sync", tag = "权限管理",
    responses((status = 200, description = "同步成功")), security(("bearer" = [])))]
pub async fn sync_api_permissions(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<PermissionSyncReport>>> {
    let report = state
        .permission_service
        .sync_api_permissions(&state.db)
        .await?;
    Ok(Json(ApiResponse::success(report)))
}
