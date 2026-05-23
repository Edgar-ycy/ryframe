use crate::dto::role_dto::{
    AssignDataScopeDto, AssignMenusDto, AssignPermsDto, CreateRoleDto, UpdateRoleDto,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, put},
};
use ryframe_common::AppResult;
use ryframe_core::PageQuery;
use ryframe_service::system::RoleVo;
use serde::{Deserialize, Serialize};
use serde_json;
use validator::Validate;

use super::auth_handler::AppState;

/// 角色列表分页查询参数（支持搜索过滤）
#[derive(Debug, Deserialize)]
pub struct RoleListQuery {
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_page_size")]
    pub page_size: u64,
    pub name: Option<String>,
    pub code: Option<String>,
    pub status: Option<String>,
}

fn default_page_size() -> u64 {
    10
}

pub fn role_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list).post(create))
        .route("/export", get(export_roles))
        .route("/{id}", get(detail).put(update))
        .route("/batch/{ids}", delete(batch_remove))
        .route("/{id}", delete(remove))
        .route("/{id}/permissions", put(assign_permissions))
        .route("/{id}/menus", put(assign_menus))
        .route("/{id}/data-scope", put(assign_data_scope))
        .with_state(state)
}

/// 角色列表分页查询
#[utoipa::path(get, path = "/api/v1/system/roles", tag = "角色管理",
    responses((status = 200, description = "角色列表")), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    Query(query): Query<RoleListQuery>,
) -> AppResult<Json<ryframe_core::PageResult<RoleVo>>> {
    let page_query = PageQuery {
        page: query.page,
        page_size: query.page_size,
    };

    // 如果有搜索条件，使用 filtered 版本
    let has_filter = query.name.is_some() || query.code.is_some() || query.status.is_some();

    if has_filter {
        state
            .role_service
            .find_by_page_filtered(
                &state.db,
                page_query,
                query.name.as_deref(),
                query.code.as_deref(),
                query.status.as_deref(),
            )
            .await
            .map(Json)
    } else {
        state
            .role_service
            .find_by_page(&state.db, page_query)
            .await
            .map(Json)
    }
}

/// 角色详情
#[utoipa::path(get, path = "/api/v1/system/roles/{id}", tag = "角色管理",
    params(("id" = i64, Path)), responses((status = 200, description = "角色详情")), security(("bearer" = [])))]
async fn detail(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<RoleVo>> {
    match state.role_service.find_by_id(&state.db, id).await? {
        Some(role) => Ok(Json(role)),
        None => Err(ryframe_common::AppError::NotFound("角色不存在".into())),
    }
}

/// 创建角色
#[utoipa::path(post, path = "/api/v1/system/roles", tag = "角色管理",
    request_body = CreateRoleDto, responses((status = 200, description = "创建成功")), security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    Json(dto): Json<CreateRoleDto>,
) -> AppResult<Json<RoleVo>> {
    dto.validate()
        .map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state
        .role_service
        .create(
            &state.db,
            &dto.name,
            &dto.code,
            dto.sort.unwrap_or(0),
            dto.data_scope,
        )
        .await
        .map(Json)
}

/// 更新角色
#[utoipa::path(put, path = "/api/v1/system/roles/{id}", tag = "角色管理",
    params(("id" = i64, Path)), request_body = UpdateRoleDto,
    responses((status = 200, description = "更新成功")), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateRoleDto>,
) -> AppResult<Json<RoleVo>> {
    dto.validate()
        .map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state
        .role_service
        .update(
            &state.db,
            id,
            &dto.name,
            dto.sort.unwrap_or(0),
            dto.status,
            dto.data_scope,
        )
        .await
        .map(Json)
}

/// 删除角色
#[utoipa::path(delete, path = "/api/v1/system/roles/{id}", tag = "角色管理",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功")), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    state.role_service.delete(&state.db, id).await?;
    Ok(Json(serde_json::json!({"message": "删除成功"})))
}

/// 批量删除角色
async fn batch_remove(
    State(state): State<AppState>,
    Path(ids_str): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let ids: Vec<i64> = ids_str
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    if ids.is_empty() {
        return Err(ryframe_common::AppError::Validation(
            "请选择要删除的角色".into(),
        ));
    }

    let count = state.role_service.delete_many(&state.db, &ids).await?;
    Ok(Json(
        serde_json::json!({"message": format!("成功删除 {} 个角色", count)}),
    ))
}

/// 导出角色数据为 Excel
async fn export_roles(State(state): State<AppState>) -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;

    // 查询所有角色
    let query = PageQuery {
        page: 1,
        page_size: 10000,
    };
    let page_result = state.role_service.find_by_page(&state.db, query).await?;

    // 转换为导出数据
    let export_data: Vec<RoleExportData> = page_result
        .records
        .into_iter()
        .map(|r| RoleExportData {
            role_id: r.id,
            role_name: r.name,
            role_code: r.code,
            data_scope: r.data_scope,
            status: r.status,
            sort: r.sort,
            remark: r.remark,
            created_at: r.created_at.to_rfc3339(),
        })
        .collect();

    // 生成 Excel
    let bytes =
        ExcelExporter::export_to_bytes(&export_data, "角色数据", &RoleExportData::excel_headers())?;

    // 返回文件
    let response = axum::response::Response::builder()
        .status(200)
        .header(
            "Content-Type",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        )
        .header("Content-Disposition", "attachment; filename=roles.xlsx")
        .body(axum::body::Body::from(bytes))
        .map_err(|e| ryframe_common::AppError::Internal(format!("构建响应失败: {}", e)))?;

    Ok(response)
}

/// 角色导出数据结构
#[derive(Debug, Serialize)]
struct RoleExportData {
    pub role_id: i64,
    pub role_name: String,
    pub role_code: String,
    pub data_scope: String,
    pub status: String,
    pub sort: i32,
    pub remark: Option<String>,
    pub created_at: String,
}

impl RoleExportData {
    fn excel_headers() -> Vec<(&'static str, &'static str)> {
        vec![
            ("role_id", "角色ID"),
            ("role_name", "角色名称"),
            ("role_code", "角色编码"),
            ("data_scope", "数据范围"),
            ("status", "状态"),
            ("sort", "排序"),
            ("remark", "备注"),
            ("created_at", "创建时间"),
        ]
    }
}

async fn assign_permissions(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<AssignPermsDto>,
) -> AppResult<Json<serde_json::Value>> {
    state
        .role_service
        .assign_permissions(&state.db, id, dto.perm_ids)
        .await?;
    Ok(Json(serde_json::json!({"message": "权限分配成功"})))
}

async fn assign_menus(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<AssignMenusDto>,
) -> AppResult<Json<serde_json::Value>> {
    state
        .role_service
        .assign_menus(&state.db, id, dto.menu_ids)
        .await?;
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
    state
        .role_service
        .assign_data_scope(&state.db, id, &dto.data_scope, dto.dept_ids)
        .await?;
    Ok(Json(serde_json::json!({"message": "数据权限设置成功"})))
}
