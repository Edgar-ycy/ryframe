use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_macro::{delete, get, post, put, route};
use ryframe_service::system::{CreateDeptCommand, DeptTreeNode, DeptVo, UpdateDeptCommand};
use validator::Validate;

use crate::dto::dept_dto::{CreateDeptDto, UpdateDeptDto};
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
        .merge(route!(list_no_page))
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
) -> AppResult<Json<ApiResponse<Vec<DeptTreeNode>>>> {
    state
        .services
        .dept
        .filter_dept_by_user(&current_user)
        .await
        .map(|v| Json(ApiResponse::success(v)))
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
) -> AppResult<Json<ApiPageResponse<DeptVo>>> {
    let (page, filter) = query.into_parts();
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
        .map(|p| Json(p.to_page_response("查询成功")))
}

/// 部门列表不分页查询（返回全部数据）
#[get("/all")]
#[perm("system:dept:list")]
#[utoipa::path(get, path = "/api/v1/system/depts/all", tag = "部门管理",
    params(DeptFilterQuery),
    responses((status = 200, description = "部门列表", body = ApiResponse<Vec<DeptVo>>)),
    security(("bearer" = [])))]
async fn list_no_page(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<DeptFilterQuery>,
) -> AppResult<Json<ApiResponse<Vec<DeptVo>>>> {
    state
        .services
        .dept
        .find_filtered(
            &current_user,
            query.name.as_deref(),
            query.status.as_deref(),
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
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
) -> AppResult<Json<ApiResponse<DeptVo>>> {
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
        .map(|v| Json(ApiResponse::success(v)))
}

/// 更新部门
#[put("/{id}")]
#[perm("system:dept:edit")]
#[utoipa::path(put, path = "/api/v1/system/depts/{id}", tag = "部门管理",
    params(("id" = i64, Path)), request_body = UpdateDeptDto,
    responses((status = 200, description = "更新成功", body = ApiResponse<DeptVo>)), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateDeptDto>,
) -> AppResult<Json<ApiResponse<DeptVo>>> {
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
        .map(|v| Json(ApiResponse::success(v)))
}

/// 部门详情
#[get("/{id}")]
#[perm("system:dept:list")]
#[utoipa::path(get, path = "/api/v1/system/depts/{id}", tag = "部门管理",
    params(("id" = i64, Path)),
    responses((status = 200, description = "部门详情", body = ApiResponse<DeptVo>)),
    security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<DeptVo>>> {
    state
        .services
        .dept
        .find_by_id(&current_user, id)
        .await?
        .ok_or_else(|| ryframe_common::AppError::NotFound("部门不存在".into()))
        .map(|value| Json(ApiResponse::success(value)))
}

/// 删除部门
#[delete("/{id}")]
#[perm("system:dept:remove")]
#[utoipa::path(delete, path = "/api/v1/system/depts/{id}", tag = "部门管理",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功", body = ryframe_common::ApiEmptyResponse)), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    remove_body!(state, current_user, id, dept)
}
