use crate::http::{ApiPageResponse, ApiResponse, HttpResult};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use ryframe_application::system::{CreateMenuCommand, MenuListParams, UpdateMenuCommand};
use ryframe_kernel::ValidatedPageQuery;
use ryframe_macro::{delete, get, post, put, route};
use validator::Validate;

use crate::RequestPrincipal;
use crate::dto::menu_dto::{CreateMenuDto, UpdateMenuDto};
use crate::dto::public_dto::{MenuTreeNode, MenuVo};
use crate::handler_utils::{parse_optional_i64, parse_optional_i64_str};
use crate::state::AppState;
use crate::{list_query, remove_body};

list_query!(pub MenuListQuery, MenuFilterQuery {
    name: String,
    status: String,
});

impl MenuFilterQuery {
    fn into_service_params(self, page: ValidatedPageQuery) -> MenuListParams {
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
        .merge(route!(list_page))
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
) -> HttpResult<Json<ApiResponse<Vec<MenuTreeNode>>>> {
    let nodes = state.services.menu.find_tree(&current_user).await?;
    let nodes = nodes
        .into_iter()
        .map(MenuTreeNode::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(ApiResponse::success(nodes)))
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
) -> HttpResult<Json<ApiPageResponse<MenuVo>>> {
    let (page, filter) = query.into_parts(&state.config.pagination)?;
    let page = state
        .services
        .menu
        .find_by_page(&current_user, filter.into_service_params(page))
        .await?;
    let records = page
        .records
        .into_iter()
        .map(MenuVo::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(ApiPageResponse::page(
        records,
        page.total,
        page.page,
        page.page_size,
        state.config.pagination.max_page_size,
    )))
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
) -> HttpResult<Json<ApiResponse<MenuVo>>> {
    dto.validate()?;
    let parent_id = parse_optional_i64(dto.parent_id)?;
    let perm_id = parse_optional_i64_str(dto.perm_id.as_deref())?;
    let value = state
        .services
        .menu
        .create(
            &current_user,
            CreateMenuCommand {
                name: dto.name,
                parent_id,
                menu_type: dto.menu_type.into(),
                perm_id,
                route_key: dto.route_key,
                icon: dto.icon,
                sort: dto.sort.unwrap_or(0),
                visible: dto.visible.unwrap_or(true),
            },
        )
        .await?;
    Ok(Json(ApiResponse::success(MenuVo::try_from(value)?)))
}

/// 更新菜单
#[put("/{id}")]
#[perm("system:menu:edit")]
#[utoipa::path(put, path = "/api/v1/system/menus/{id}", tag = "菜单管理",
    params(("id" = String, Path)), request_body = UpdateMenuDto,
    responses((status = 200, description = "更新成功", body = ApiResponse<MenuVo>)), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateMenuDto>,
) -> HttpResult<Json<ApiResponse<MenuVo>>> {
    dto.validate()?;
    let parent_id = parse_optional_i64(dto.parent_id)?;
    let perm_id = parse_optional_i64_str(dto.perm_id.as_deref())?;
    let value = state
        .services
        .menu
        .update(
            &current_user,
            UpdateMenuCommand {
                id,
                name: dto.name,
                parent_id,
                menu_type: dto.menu_type.into(),
                perm_id,
                route_key: dto.route_key,
                icon: dto.icon,
                sort: dto.sort.unwrap_or(0),
                visible: dto.visible.unwrap_or(true),
                status: dto.status,
            },
        )
        .await?;
    Ok(Json(ApiResponse::success(MenuVo::try_from(value)?)))
}

/// 菜单详情
#[get("/{id}")]
#[perm("system:menu:list")]
#[utoipa::path(get, path = "/api/v1/system/menus/{id}", tag = "菜单管理",
    params(("id" = String, Path)),
    responses((status = 200, description = "菜单详情", body = ApiResponse<MenuVo>)),
    security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> HttpResult<Json<ApiResponse<MenuVo>>> {
    let menu = state
        .services
        .menu
        .find_by_id(&current_user, id)
        .await?
        .ok_or_else(|| ryframe_kernel::AppError::NotFound("菜单不存在".into()))?;
    Ok(Json(ApiResponse::success(MenuVo::try_from(menu)?)))
}

/// 删除菜单
#[delete("/{id}")]
#[perm("system:menu:remove")]
#[utoipa::path(delete, path = "/api/v1/system/menus/{id}", tag = "菜单管理",
    params(("id" = String, Path)), responses((status = 200, description = "删除成功", body = crate::http::ApiEmptyResponse)), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> HttpResult<Json<ApiResponse<()>>> {
    remove_body!(state, current_user, id, menu)
}
