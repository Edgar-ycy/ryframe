use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_db::repositories::dept_repo::DeptTreeNode;
use ryframe_service::system::DeptVo;
use serde::Deserialize;
use validator::Validate;

use super::auth_handler::AppState;
use crate::dto::dept_dto::{CreateDeptDto, UpdateDeptDto};

/// 部门列表查询参数（支持搜索过滤）
#[derive(Debug, Deserialize)]
pub struct DeptListQuery {
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_page_size", alias = "pageSize")]
    pub page_size: u64,
    pub name: Option<String>,
    pub status: Option<String>,
}

fn default_page_size() -> u64 {
    10
}

pub fn dept_router(state: AppState) -> Router {
    Router::new()
        .route("/tree", get(tree))
        .route("/list", get(list_page))
        .route("/listNoPage", get(list_no_page))
        .route("/", post(create))
        .route("/{id}", get(detail).put(update).delete(remove))
        .with_state(state)
}

/// 部门树查询
#[utoipa::path(get, path = "/api/v1/system/depts/tree", tag = "部门管理",
    responses((status = 200, description = "部门树")), security(("bearer" = [])))]
async fn tree(State(state): State<AppState>) -> AppResult<Json<ApiResponse<Vec<DeptTreeNode>>>> {
    state
        .dept_service
        .find_tree(&state.db)
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 部门列表分页查询
#[utoipa::path(get, path = "/api/v1/system/depts/list", tag = "部门管理",
    responses((status = 200, description = "部门列表")),
    security(("bearer" = [])))]
async fn list_page(
    State(state): State<AppState>,
    Query(query): Query<DeptListQuery>,
) -> AppResult<Json<ApiPageResponse<DeptVo>>> {
    let all = state
        .dept_service
        .find_filtered(&state.db, query.name.as_deref(), query.status.as_deref())
        .await?;
    let total = all.len() as u64;
    let offset = ((query.page.saturating_sub(1)) * query.page_size) as usize;
    let rows: Vec<DeptVo> = all
        .into_iter()
        .skip(offset)
        .take(query.page_size as usize)
        .collect();
    Ok(Json(ApiPageResponse::new(rows, total, "查询成功")))
}

/// 部门列表不分页查询（返回全部数据）
#[utoipa::path(get, path = "/api/v1/system/depts/listNoPage", tag = "部门管理",
    responses((status = 200, description = "部门列表")),
    security(("bearer" = [])))]
async fn list_no_page(
    State(state): State<AppState>,
    Query(query): Query<DeptListQuery>,
) -> AppResult<Json<ApiResponse<Vec<DeptVo>>>> {
    state
        .dept_service
        .find_filtered(&state.db, query.name.as_deref(), query.status.as_deref())
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 创建部门
#[utoipa::path(post, path = "/api/v1/system/depts", tag = "部门管理",
    request_body = CreateDeptDto, responses((status = 200, description = "创建成功")), security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    Json(dto): Json<CreateDeptDto>,
) -> AppResult<Json<ApiResponse<ryframe_db::entities::dept::Model>>> {
    dto.validate()
        .map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state
        .dept_service
        .create(&state.db, &dto.name, dto.parent_id, dto.sort.unwrap_or(0))
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 更新部门
#[utoipa::path(put, path = "/api/v1/system/depts/{id}", tag = "部门管理",
    params(("id" = i64, Path)), request_body = UpdateDeptDto,
    responses((status = 200, description = "更新成功")), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateDeptDto>,
) -> AppResult<Json<ApiResponse<ryframe_db::entities::dept::Model>>> {
    dto.validate()
        .map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state
        .dept_service
        .update(
            &state.db,
            id,
            &dto.name,
            dto.parent_id,
            dto.sort.unwrap_or(0),
            dto.status,
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 部门详情
#[utoipa::path(get, path = "/api/v1/system/depts/{id}", tag = "部门管理",
    params(("id" = i64, Path)),
    responses((status = 200, description = "部门详情")),
    security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<DeptVo>>> {
    match state.dept_service.find_by_id(&state.db, id).await? {
        Some(dept) => Ok(Json(ApiResponse::success(dept))),
        None => Err(ryframe_common::AppError::NotFound("部门不存在".into())),
    }
}

/// 删除部门
#[utoipa::path(delete, path = "/api/v1/system/depts/{id}", tag = "部门管理",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功")), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    state.dept_service.delete(&state.db, id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("删除成功")))
}
