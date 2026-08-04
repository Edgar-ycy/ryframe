use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use ryframe_http::{ApiPageResponse, ApiResponse, HttpResult};
use ryframe_macro::{delete, get, post, put, route};
use ryframe_service::system::{CreateDeptCommand, UpdateDeptCommand};
use validator::Validate;

use crate::dto::dept_dto::{CreateDeptDto, UpdateDeptDto};
use crate::dto::public_dto::{DeptTreeNode, DeptVo};
use crate::handler_utils::parse_optional_i64;
use crate::state::AppState;
use crate::{list_query, remove_body};
use ryframe_auth::RequestPrincipal;

list_query!(pub DeptListQuery, DeptFilterQuery {
    name: String,
    status: String,
});

pub fn dept_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(tree))
        .merge(route!(list_page))
        .merge(route!(detail))
        .merge(route!(create))
        .merge(route!(update))
        .merge(route!(remove))
        .with_state(state)
}

/// 部门树查询
#[get("/tree")]
#[perm("system:dept:list")]
#[utoipa::path(get, path = "/api/v1/system/depts/tree", tag = "部门管理",
    responses((status = 200, description = "部门树", body = ApiResponse<Vec<DeptTreeNode>>)), security(("bearer" = [])))]
async fn tree(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> HttpResult<Json<ApiResponse<Vec<DeptTreeNode>>>> {
    state
        .services
        .dept
        .filter_dept_by_user(&current_user)
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|nodes| {
            Json(ApiResponse::success(
                nodes.into_iter().map(DeptTreeNode::from).collect(),
            ))
        })
}

/// 部门列表分页查询
#[get("/")]
#[perm("system:dept:list")]
#[utoipa::path(get, path = "/api/v1/system/depts", tag = "部门管理",
    params(DeptListQuery),
    responses((status = 200, description = "部门列表", body = ApiPageResponse<DeptVo>)),
    security(("bearer" = [])))]
async fn list_page(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<DeptListQuery>,
) -> HttpResult<Json<ApiPageResponse<DeptVo>>> {
    let (page, filter) = query.into_parts(&state.config.pagination)?;
    state
        .services
        .dept
        .find_by_page_filtered(
            &current_user,
            page,
            filter.name.as_deref(),
            filter.status.as_deref(),
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|p| {
            Json(ApiPageResponse::new(
                p.records.into_iter().map(DeptVo::from).collect(),
                p.total,
                p.page,
                p.page_size,
                state.config.pagination.max_page_size,
                "查询成功",
            ))
        })
}

/// 创建部门
#[post("/")]
#[perm("system:dept:add")]
#[utoipa::path(post, path = "/api/v1/system/depts", tag = "部门管理",
    request_body = CreateDeptDto, responses((status = 200, description = "创建成功", body = ApiResponse<DeptVo>)), security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(dto): Json<CreateDeptDto>,
) -> HttpResult<Json<ApiResponse<DeptVo>>> {
    dto.validate()?;
    let parent_id = parse_optional_i64(dto.parent_id)?;
    state
        .services
        .dept
        .create(
            &current_user,
            CreateDeptCommand {
                name: dto.name,
                parent_id,
                sort: dto.sort.unwrap_or(0),
            },
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|value| Json(ApiResponse::success(value.into())))
}

/// 更新部门
#[put("/{id}")]
#[perm("system:dept:edit")]
#[utoipa::path(put, path = "/api/v1/system/depts/{id}", tag = "部门管理",
    params(("id" = String, Path)), request_body = UpdateDeptDto,
    responses((status = 200, description = "更新成功", body = ApiResponse<DeptVo>)), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateDeptDto>,
) -> HttpResult<Json<ApiResponse<DeptVo>>> {
    dto.validate()?;
    let parent_id = parse_optional_i64(dto.parent_id)?;
    state
        .services
        .dept
        .update(
            &current_user,
            UpdateDeptCommand {
                id,
                name: dto.name,
                parent_id,
                sort: dto.sort.unwrap_or(0),
                status: dto.status,
            },
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|value| Json(ApiResponse::success(value.into())))
}

/// 部门详情
#[get("/{id}")]
#[perm("system:dept:list")]
#[utoipa::path(get, path = "/api/v1/system/depts/{id}", tag = "部门管理",
    params(("id" = String, Path)),
    responses((status = 200, description = "部门详情", body = ApiResponse<DeptVo>)),
    security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> HttpResult<Json<ApiResponse<DeptVo>>> {
    let value = state
        .services
        .dept
        .find_by_id(&current_user, id)
        .await?
        .ok_or_else(|| ryframe_kernel::AppError::NotFound("部门不存在".into()))?;
    Ok(Json(ApiResponse::success(value.into())))
}

/// 删除部门
#[delete("/{id}")]
#[perm("system:dept:remove")]
#[utoipa::path(delete, path = "/api/v1/system/depts/{id}", tag = "部门管理",
    params(("id" = String, Path)), responses((status = 200, description = "删除成功", body = ryframe_http::ApiEmptyResponse)), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> HttpResult<Json<ApiResponse<()>>> {
    remove_body!(state, current_user, id, dept)
}
