use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, post, put},
};
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_db::{entities::menu, repositories::menu_repo::MenuTreeNode};
use validator::Validate;

use super::auth_handler::AppState;
use crate::dto::menu_dto::{CreateMenuDto, UpdateMenuDto};
use crate::extractors::CurrentUser;
use crate::{detail_body, list_query, remove_body};

list_query!(pub MenuListQuery {
    name: String,
    status: String,
});

pub fn menu_router(state: AppState) -> Router {
    Router::new()
        .route("/tree", get(tree))
        .route("/user-tree", get(user_tree))
        .route("/list", get(list_page))
        .route("/listNoPage", get(list_no_page))
        .route("/", post(create))
        .route("/{id}", get(detail))
        .route("/{id}", put(update))
        .route("/{id}", delete(remove))
        .with_state(state)
}

/// 菜单树查询
#[utoipa::path(get, path = "/api/v1/system/menus/tree", tag = "菜单管理",
    responses((status = 200, description = "菜单树")), security(("bearer" = [])))]
async fn tree(State(state): State<AppState>) -> AppResult<Json<ApiResponse<Vec<MenuTreeNode>>>> {
    state
        .menu_service
        .find_tree(&state.db)
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 当前用户可见的菜单树（按角色过滤，前端用）
#[utoipa::path(get, path = "/api/v1/system/menus/user-tree", tag = "菜单管理",
    responses((status = 200, description = "用户菜单树")), security(("bearer" = [])))]
async fn user_tree(
    State(state): State<AppState>,
    Extension(current_user): Extension<CurrentUser>,
) -> AppResult<Json<ApiResponse<Vec<MenuTreeNode>>>> {
    let tree = if current_user.is_super_admin {
        // 超级管理员看全部菜单树
        state.menu_service.find_tree(&state.db).await?
    } else if current_user.role_ids.is_empty() {
        vec![]
    } else {
        state
            .menu_service
            .find_tree_by_roles(&state.db, &current_user.role_ids)
            .await?
    };
    Ok(Json(ApiResponse::success(tree)))
}

/// 菜单列表分页查询
#[utoipa::path(get, path = "/api/v1/system/menus/list", tag = "菜单管理",
    responses((status = 200, description = "菜单列表")),
    security(("bearer" = [])))]
async fn list_page(
    State(state): State<AppState>,
    Query(query): Query<MenuListQuery>,
) -> AppResult<Json<ApiPageResponse<menu::Model>>> {
    let all = state
        .menu_service
        .find_filtered(&state.db, query.name.as_deref(), query.status.as_deref())
        .await?;
    let total = all.len() as u64;
    let offset = ((query.page.saturating_sub(1)) * query.page_size) as usize;
    let rows: Vec<menu::Model> = all
        .into_iter()
        .skip(offset)
        .take(query.page_size as usize)
        .collect();
    Ok(Json(ApiPageResponse::new(rows, total, "查询成功")))
}

/// 菜单列表不分页查询（返回全部数据）
#[utoipa::path(get, path = "/api/v1/system/menus/listNoPage", tag = "菜单管理",
    responses((status = 200, description = "菜单列表")),
    security(("bearer" = [])))]
async fn list_no_page(
    State(state): State<AppState>,
    Query(query): Query<MenuListQuery>,
) -> AppResult<Json<ApiResponse<Vec<menu::Model>>>> {
    state
        .menu_service
        .find_filtered(&state.db, query.name.as_deref(), query.status.as_deref())
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 创建菜单
#[utoipa::path(post, path = "/api/v1/system/menus", tag = "菜单管理",
    request_body = CreateMenuDto, responses((status = 200, description = "创建成功")), security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    Json(dto): Json<CreateMenuDto>,
) -> AppResult<Json<ApiResponse<ryframe_db::entities::menu::Model>>> {
    dto.validate()?;
    let parent_id: Option<i64> = dto.parent_id.and_then(|s| s.parse().ok());
    state
        .menu_service
        .create(
            &state.db,
            &dto.name,
            parent_id,
            &dto.menu_type,
            dto.path.as_deref(),
            dto.component.as_deref(),
            dto.query.as_deref(),
            dto.perms.as_deref(),
            dto.icon.as_deref(),
            dto.is_frame.unwrap_or(false),
            dto.is_cache.unwrap_or(false),
            dto.sort.unwrap_or(0),
            dto.visible.unwrap_or(true),
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 更新菜单
#[utoipa::path(put, path = "/api/v1/system/menus/{id}", tag = "菜单管理",
    params(("id" = i64, Path)), request_body = UpdateMenuDto,
    responses((status = 200, description = "更新成功")), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateMenuDto>,
) -> AppResult<Json<ApiResponse<ryframe_db::entities::menu::Model>>> {
    dto.validate()?;
    let parent_id: Option<i64> = dto.parent_id.and_then(|s| s.parse().ok());
    state
        .menu_service
        .update(
            &state.db,
            id,
            &dto.name,
            parent_id,
            &dto.menu_type,
            dto.path.as_deref(),
            dto.component.as_deref(),
            dto.query.as_deref(),
            dto.perms.as_deref(),
            dto.icon.as_deref(),
            dto.is_frame.unwrap_or(false),
            dto.is_cache.unwrap_or(false),
            dto.sort.unwrap_or(0),
            dto.visible.unwrap_or(true),
            dto.status,
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 菜单详情
#[utoipa::path(get, path = "/api/v1/system/menus/{id}", tag = "菜单管理",
    params(("id" = i64, Path)),
    responses((status = 200, description = "菜单详情")),
    security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<menu::Model>>> {
    detail_body!(state, id, menu_service, menu::Model, "菜单")
}

/// 删除菜单
#[utoipa::path(delete, path = "/api/v1/system/menus/{id}", tag = "菜单管理",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功")), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    remove_body!(state, id, menu_service)
}
