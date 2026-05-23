use crate::dto::menu_dto::{CreateMenuDto, UpdateMenuDto};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use ryframe_common::AppResult;
use ryframe_db::entities::menu;
use ryframe_db::repositories::menu_repo::MenuTreeNode;
use serde::Deserialize;
use serde_json;
use validator::Validate;

use super::auth_handler::AppState;

/// 菜单列表查询参数
#[derive(Debug, Deserialize)]
pub struct MenuListQuery {
    pub name: Option<String>,
    pub status: Option<String>,
}

pub fn menu_router(state: AppState) -> Router {
    Router::new()
        .route("/tree", get(tree))
        .route("/list", get(list))
        .route("/", post(create))
        .route("/{id}", get(detail).put(update).delete(remove))
        .with_state(state)
}

/// 菜单树查询
#[utoipa::path(get, path = "/api/v1/system/menus/tree", tag = "菜单管理",
    responses((status = 200, description = "菜单树")), security(("bearer" = [])))]
async fn tree(State(state): State<AppState>) -> AppResult<Json<Vec<MenuTreeNode>>> {
    state.menu_service.find_tree(&state.db).await.map(Json)
}

/// 菜单列表（支持按名称/状态搜索）
async fn list(
    State(state): State<AppState>,
    Query(query): Query<MenuListQuery>,
) -> AppResult<Json<Vec<menu::Model>>> {
    state
        .menu_service
        .find_filtered(&state.db, query.name.as_deref(), query.status.as_deref())
        .await
        .map(Json)
}

/// 创建菜单
#[utoipa::path(post, path = "/api/v1/system/menus", tag = "菜单管理",
    request_body = CreateMenuDto, responses((status = 200, description = "创建成功")), security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    Json(dto): Json<CreateMenuDto>,
) -> AppResult<Json<ryframe_db::entities::menu::Model>> {
    dto.validate()
        .map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state
        .menu_service
        .create(
            &state.db,
            &dto.name,
            dto.parent_id,
            dto.path.as_deref(),
            dto.component.as_deref(),
            dto.icon.as_deref(),
            dto.sort.unwrap_or(0),
            dto.visible.unwrap_or(true),
        )
        .await
        .map(Json)
}

/// 更新菜单
#[utoipa::path(put, path = "/api/v1/system/menus/{id}", tag = "菜单管理",
    params(("id" = i64, Path)), request_body = UpdateMenuDto,
    responses((status = 200, description = "更新成功")), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateMenuDto>,
) -> AppResult<Json<ryframe_db::entities::menu::Model>> {
    dto.validate()
        .map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state
        .menu_service
        .update(
            &state.db,
            id,
            &dto.name,
            dto.parent_id,
            dto.path.as_deref(),
            dto.component.as_deref(),
            dto.icon.as_deref(),
            dto.sort.unwrap_or(0),
            dto.visible.unwrap_or(true),
            dto.status,
        )
        .await
        .map(Json)
}

/// 菜单详情
async fn detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<menu::Model>> {
    match state.menu_service.find_by_id(&state.db, id).await? {
        Some(menu) => Ok(Json(menu)),
        None => Err(ryframe_common::AppError::NotFound("菜单不存在".into())),
    }
}

/// 删除菜单
#[utoipa::path(delete, path = "/api/v1/system/menus/{id}", tag = "菜单管理",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功")), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    state.menu_service.delete(&state.db, id).await?;
    Ok(Json(serde_json::json!({"message": "删除成功"})))
}
