use serde_json;
use axum::{extract::{Path, Query, State}, routing::{get, put}, Json, Router};
use ryframe_common::AppResult;
use ryframe_core::PageQuery;
use ryframe_service::system::RoleVo;
use validator::Validate;
use crate::dto::role_dto::{AssignDataScopeDto, AssignMenusDto, AssignPermsDto, CreateRoleDto, UpdateRoleDto};

use super::auth_handler::AppState;

pub fn role_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", get(detail).put(update).delete(remove))
        .route("/{id}/permissions", put(assign_permissions))
        .route("/{id}/menus", put(assign_menus))
        .route("/{id}/data-scope", put(assign_data_scope))
        .with_state(state)
}

async fn list(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> AppResult<Json<ryframe_core::PageResult<RoleVo>>> {
    state.role_service.find_by_page(&state.db, query).await.map(Json)
}

async fn detail(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<RoleVo>> {
    match state.role_service.find_by_id(&state.db, id).await? {
        Some(role) => Ok(Json(role)),
        None => Err(ryframe_common::AppError::NotFound("角色不存在".into())),
    }
}

async fn create(
    State(state): State<AppState>,
    Json(dto): Json<CreateRoleDto>,
) -> AppResult<Json<RoleVo>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.role_service.create(&state.db, &dto.name, &dto.code, dto.sort.unwrap_or(0), dto.data_scope).await.map(Json)
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateRoleDto>,
) -> AppResult<Json<RoleVo>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.role_service.update(&state.db, id, &dto.name, dto.sort.unwrap_or(0), dto.status, dto.data_scope).await.map(Json)
}

async fn remove(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<serde_json::Value>> {
    state.role_service.delete(&state.db, id).await?;
    Ok(Json(serde_json::json!({"message": "删除成功"})))
}

async fn assign_permissions(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<AssignPermsDto>,
) -> AppResult<Json<serde_json::Value>> {
    state.role_service.assign_permissions(&state.db, id, dto.perm_ids).await?;
    Ok(Json(serde_json::json!({"message": "权限分配成功"})))
}

async fn assign_menus(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<AssignMenusDto>,
) -> AppResult<Json<serde_json::Value>> {
    state.role_service.assign_menus(&state.db, id, dto.menu_ids).await?;
    Ok(Json(serde_json::json!({"message": "菜单分配成功"})))
}

/// 设置角色数据权限
///
/// PUT /api/v1/system/roles/{id}/data-scope
async fn assign_data_scope(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<AssignDataScopeDto>,
) -> AppResult<Json<serde_json::Value>> {
    state.role_service.assign_data_scope(&state.db, id, &dto.data_scope, dto.dept_ids).await?;
    Ok(Json(serde_json::json!({"message": "数据权限设置成功"})))
}
