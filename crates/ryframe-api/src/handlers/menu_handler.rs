use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_core::PageQuery;
use ryframe_macro::{delete, get, post, put, route};
use ryframe_service::system::{
    CreateMenuCommand, MenuListParams, MenuTreeNode, MenuVo, UpdateMenuCommand,
};
use validator::Validate;

use crate::dto::menu_dto::{CreateMenuDto, UpdateMenuDto};
use crate::handler_utils::{parse_optional_i64, parse_optional_i64_str};
use crate::state::AppState;
use crate::{list_query, remove_body};
use ryframe_auth::RequestPrincipal;

list_query!(pub MenuListQuery, MenuFilterQuery {
    name: String,
    status: String,
});

impl MenuFilterQuery {
    fn into_service_params(self, page: PageQuery) -> MenuListParams {
        MenuListParams {
            page,
            name: self.name,
            status: self.status,
        }
    }
}

pub fn menu_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(tree))
        .merge(route!(user_tree))
        .merge(route!(list_page))
        .merge(route!(list_no_page))
        .merge(route!(create))
        .merge(route!(detail))
        .merge(route!(update))
        .merge(route!(remove))
        .with_state(state)
}

/// 菜单树查询
#[get("/tree")]
#[perm("system:menu:list")]
#[utoipa::path(get, path = "/api/v1/system/menus/tree", tag = "菜单管理",
    responses((status = 200, description = "菜单树", body = ApiResponse<Vec<MenuTreeNode>>)), security(("bearer" = [])))]
async fn tree(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> AppResult<Json<ApiResponse<Vec<MenuTreeNode>>>> {
    state
        .services
        .menu
        .find_tree(&current_user)
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 当前用户可见的菜单树（按角色过滤，前端用）
#[get("/current")]
#[utoipa::path(get, path = "/api/v1/system/menus/current", tag = "菜单管理",
    responses((status = 200, description = "用户菜单树", body = ApiResponse<Vec<MenuTreeNode>>)), security(("bearer" = [])))]
pub async fn user_tree(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> AppResult<Json<ApiResponse<Vec<MenuTreeNode>>>> {
    let tree = if current_user.is_super_admin {
        // 超级管理员看全部菜单树
        state.services.menu.find_tree(&current_user).await?
    } else if current_user.role_ids.is_empty() {
        vec![]
    } else {
        state
            .services
            .menu
            .find_tree_by_permissions(&current_user, &current_user.permissions)
            .await?
    };
    Ok(Json(ApiResponse::success(tree)))
}

/// 菜单列表分页查询
#[get("/")]
#[perm("system:menu:list")]
#[utoipa::path(get, path = "/api/v1/system/menus", tag = "菜单管理",
    params(MenuListQuery),
    responses((status = 200, description = "菜单列表", body = ApiPageResponse<MenuVo>)),
    security(("bearer" = [])))]
async fn list_page(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<MenuListQuery>,
) -> AppResult<Json<ApiPageResponse<MenuVo>>> {
    let (page, filter) = query.into_parts();
    state
        .services
        .menu
        .find_by_page(&current_user, filter.into_service_params(page))
        .await
        .map(|page| Json(page.to_page_response("查询成功")))
}

/// 菜单列表不分页查询（返回全部数据）
#[get("/all")]
#[perm("system:menu:list")]
#[utoipa::path(get, path = "/api/v1/system/menus/all", tag = "菜单管理",
    params(MenuFilterQuery),
    responses((status = 200, description = "菜单列表", body = ApiResponse<Vec<MenuVo>>)),
    security(("bearer" = [])))]
async fn list_no_page(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<MenuFilterQuery>,
) -> AppResult<Json<ApiResponse<Vec<MenuVo>>>> {
    state
        .services
        .menu
        .find_by_page(
            &current_user,
            query.into_service_params(PageQuery::all_records()),
        )
        .await
        .map(|page| Json(ApiResponse::success(page.records)))
}

/// 创建菜单
#[post("/")]
#[perm("system:menu:add")]
#[utoipa::path(post, path = "/api/v1/system/menus", tag = "菜单管理",
    request_body = CreateMenuDto, responses((status = 200, description = "创建成功", body = ApiResponse<MenuVo>)), security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(dto): Json<CreateMenuDto>,
) -> AppResult<Json<ApiResponse<MenuVo>>> {
    dto.validate()?;
    let parent_id = parse_optional_i64(dto.parent_id)?;
    let perm_id = parse_optional_i64_str(dto.perm_id.as_deref())?;
    state
        .services
        .menu
        .create(
            &current_user,
            CreateMenuCommand {
                name: dto.name,
                parent_id,
                menu_type: dto.menu_type,
                perm_id,
                route_key: dto.route_key,
                icon: dto.icon,
                sort: dto.sort.unwrap_or(0),
                visible: dto.visible.unwrap_or(true),
            },
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 更新菜单
#[put("/{id}")]
#[perm("system:menu:edit")]
#[utoipa::path(put, path = "/api/v1/system/menus/{id}", tag = "菜单管理",
    params(("id" = i64, Path)), request_body = UpdateMenuDto,
    responses((status = 200, description = "更新成功", body = ApiResponse<MenuVo>)), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateMenuDto>,
) -> AppResult<Json<ApiResponse<MenuVo>>> {
    dto.validate()?;
    let parent_id = parse_optional_i64(dto.parent_id)?;
    let perm_id = parse_optional_i64_str(dto.perm_id.as_deref())?;
    state
        .services
        .menu
        .update(
            &current_user,
            UpdateMenuCommand {
                id,
                name: dto.name,
                parent_id,
                menu_type: dto.menu_type,
                perm_id,
                route_key: dto.route_key,
                icon: dto.icon,
                sort: dto.sort.unwrap_or(0),
                visible: dto.visible.unwrap_or(true),
                status: dto.status,
            },
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 菜单详情
#[get("/{id}")]
#[perm("system:menu:list")]
#[utoipa::path(get, path = "/api/v1/system/menus/{id}", tag = "菜单管理",
    params(("id" = i64, Path)),
    responses((status = 200, description = "菜单详情", body = ApiResponse<MenuVo>)),
    security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<MenuVo>>> {
    state
        .services
        .menu
        .find_by_id(&current_user, id)
        .await?
        .ok_or_else(|| ryframe_common::AppError::NotFound("菜单不存在".into()))
        .map(|menu| Json(ApiResponse::success(menu)))
}

/// 删除菜单
#[delete("/{id}")]
#[perm("system:menu:remove")]
#[utoipa::path(delete, path = "/api/v1/system/menus/{id}", tag = "菜单管理",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功", body = ryframe_common::ApiEmptyResponse)), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    remove_body!(state, current_user, id, menu)
}
