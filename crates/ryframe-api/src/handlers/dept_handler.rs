use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_core::PageQuery;
use ryframe_db::repositories::dept_repo::DeptTreeNode;
use ryframe_macro::{delete, get, post, put, route};
use ryframe_service::system::DeptVo;
use validator::Validate;

use super::auth_handler::AppState;
use crate::dto::dept_dto::{CreateDeptDto, UpdateDeptDto};
use crate::extractors::CurrentUser;
use crate::handler_utils::parse_optional_i64;
use crate::{list_query, remove_body};

list_query!(pub DeptListQuery {
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
    responses((status = 200, description = "部门树")), security(("bearer" = [])))]
async fn tree(
    State(state): State<AppState>,
    current_user: CurrentUser,
) -> AppResult<Json<ApiResponse<Vec<DeptTreeNode>>>> {
    state
        .dept_service
        .filter_dept_by_user(&state.db, &current_user.to_data_scope_context())
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 部门列表分页查询
#[get("/list")]
#[perm("system:dept:list")]
#[utoipa::path(get, path = "/api/v1/system/depts/list", tag = "部门管理",
    responses((status = 200, description = "部门列表")),
    security(("bearer" = [])))]
async fn list_page(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Query(query): Query<DeptListQuery>,
) -> AppResult<Json<ApiPageResponse<DeptVo>>> {
    state
        .dept_service
        .find_by_page_filtered_with_data_scope(
            &state.db,
            PageQuery {
                page: query.page,
                page_size: query.page_size,
            },
            query.name.as_deref(),
            query.status.as_deref(),
            &current_user.to_data_scope_context(),
        )
        .await
        .map(|p| Json(p.to_page_response("查询成功")))
}

/// 部门列表不分页查询（返回全部数据）
#[get("/listNoPage")]
#[perm("system:dept:list")]
#[utoipa::path(get, path = "/api/v1/system/depts/listNoPage", tag = "部门管理",
    responses((status = 200, description = "部门列表")),
    security(("bearer" = [])))]
async fn list_no_page(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Query(query): Query<DeptListQuery>,
) -> AppResult<Json<ApiResponse<Vec<DeptVo>>>> {
    state
        .dept_service
        .find_filtered_with_data_scope(
            &state.db,
            query.name.as_deref(),
            query.status.as_deref(),
            &current_user.to_data_scope_context(),
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 创建部门
#[post("/")]
#[perm("system:dept:add")]
#[utoipa::path(post, path = "/api/v1/system/depts", tag = "部门管理",
    request_body = CreateDeptDto, responses((status = 200, description = "创建成功")), security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    Json(dto): Json<CreateDeptDto>,
) -> AppResult<Json<ApiResponse<ryframe_db::entities::dept::Model>>> {
    dto.validate()?;
    let parent_id = parse_optional_i64(dto.parent_id);
    state
        .dept_service
        .create(&state.db, &dto.name, parent_id, dto.sort.unwrap_or(0))
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 更新部门
#[put("/{id}")]
#[perm("system:dept:edit")]
#[utoipa::path(put, path = "/api/v1/system/depts/{id}", tag = "部门管理",
    params(("id" = i64, Path)), request_body = UpdateDeptDto,
    responses((status = 200, description = "更新成功")), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateDeptDto>,
) -> AppResult<Json<ApiResponse<ryframe_db::entities::dept::Model>>> {
    dto.validate()?;
    let parent_id = parse_optional_i64(dto.parent_id);
    state
        .dept_service
        .update(
            &state.db,
            id,
            &dto.name,
            parent_id,
            dto.sort.unwrap_or(0),
            dto.status,
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 部门详情
#[get("/{id}")]
#[perm("system:dept:list")]
#[utoipa::path(get, path = "/api/v1/system/depts/{id}", tag = "部门管理",
    params(("id" = i64, Path)),
    responses((status = 200, description = "部门详情")),
    security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<DeptVo>>> {
    state
        .dept_service
        .find_by_id_with_data_scope(&state.db, id, &current_user.to_data_scope_context())
        .await?
        .ok_or_else(|| ryframe_common::AppError::NotFound("部门不存在".into()))
        .map(|value| Json(ApiResponse::success(value)))
}

/// 删除部门
#[delete("/{id}")]
#[perm("system:dept:remove")]
#[utoipa::path(delete, path = "/api/v1/system/depts/{id}", tag = "部门管理",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功")), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    remove_body!(state, id, dept_service)
}
