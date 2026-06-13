use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, post, put},
};
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_core::PageQuery;
use ryframe_service::system::RoleVo;
use serde::Serialize;
use validator::Validate;

use super::auth_handler::AppState;
use crate::dto::role_dto::{
    AssignDataScopeDto, AssignMenusDto, AssignPermsDto, CreateRoleDto, UpdateRoleDto,
};
use crate::{detail_body, list_query, remove_body};

list_query!(pub RoleListQuery {
    name: String,
    code: String,
    status: String,
});

pub fn role_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list))
        .route("/", post(create))
        .route("/list", get(list))
        .route("/listNoPage", get(list_no_page))
        .route("/export", get(export_roles))
        .route("/{id}", get(detail))
        .route("/{id}", put(update))
        .route("/{id}", delete(remove))
        .route("/batch/{ids}", delete(batch_remove))
        .route("/{id}/permissions", get(get_role_perms))
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
) -> AppResult<Json<ApiPageResponse<RoleVo>>> {
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
            .map(|p| Json(p.to_page_response("查询成功")))
    } else {
        state
            .role_service
            .find_by_page(&state.db, page_query)
            .await
            .map(|p| Json(p.to_page_response("查询成功")))
    }
}

/// 角色列表不分页查询（返回全部数据）
#[utoipa::path(get, path = "/api/v1/system/roles/listNoPage", tag = "角色管理",
    responses((status = 200, description = "角色列表")),
    security(("bearer" = [])))]
async fn list_no_page(State(state): State<AppState>) -> AppResult<Json<ApiResponse<Vec<RoleVo>>>> {
    let page_query = PageQuery {
        page: 1,
        page_size: 10000,
    };
    state
        .role_service
        .find_by_page(&state.db, page_query)
        .await
        .map(|p| Json(ApiResponse::success(p.records)))
}

/// 角色详情
#[utoipa::path(get, path = "/api/v1/system/roles/{id}", tag = "角色管理",
    params(("id" = i64, Path)), responses((status = 200, description = "角色详情")), security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<RoleVo>>> {
    detail_body!(state, id, role_service, RoleVo, "角色")
}

/// 创建角色
#[utoipa::path(post, path = "/api/v1/system/roles", tag = "角色管理",
    request_body = CreateRoleDto, responses((status = 200, description = "创建成功")), security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    Json(dto): Json<CreateRoleDto>,
) -> AppResult<Json<ApiResponse<RoleVo>>> {
    dto.validate()?;
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
        .map(|v| Json(ApiResponse::success(v)))
}

/// 更新角色
#[utoipa::path(put, path = "/api/v1/system/roles/{id}", tag = "角色管理",
    params(("id" = i64, Path)), request_body = UpdateRoleDto,
    responses((status = 200, description = "更新成功")), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateRoleDto>,
) -> AppResult<Json<ApiResponse<RoleVo>>> {
    dto.validate()?;
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
        .map(|v| Json(ApiResponse::success(v)))
}

/// 删除角色
#[utoipa::path(delete, path = "/api/v1/system/roles/{id}", tag = "角色管理",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功")), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    remove_body!(state, id, role_service)
}

/// 批量删除角色
#[utoipa::path(delete, path = "/api/v1/system/roles/batch/{ids}", tag = "角色管理",
    params(("ids" = String, Path)),
    responses((status = 200, description = "批量删除成功")),
    security(("bearer" = [])))]
async fn batch_remove(
    State(state): State<AppState>,
    Path(ids_str): Path<String>,
) -> AppResult<Json<ApiResponse<()>>> {
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
    Ok(Json(ApiResponse::success_no_data_with_msg(format!(
        "成功删除 {} 个角色",
        count
    ))))
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
    pub role_id: String,
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

/// 分配权限给角色
#[utoipa::path(put, path = "/api/v1/system/roles/{id}/permissions", tag = "角色管理",
    params(("id" = i64, Path)),
    request_body = AssignPermsDto,
    responses((status = 200, description = "权限分配成功")),
    security(("bearer" = [])))]
async fn assign_permissions(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<AssignPermsDto>,
) -> AppResult<Json<ApiResponse<()>>> {
    let perm_ids: Vec<i64> = dto.perm_ids.iter().filter_map(|s| s.parse().ok()).collect();
    state
        .role_service
        .assign_permissions(&state.db, id, perm_ids)
        .await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("权限分配成功")))
}

/// 查询角色已分配的权限ID列表
#[utoipa::path(get, path = "/api/v1/system/roles/{id}/permissions", tag = "角色管理",
    params(("id" = i64, Path)),
    responses((status = 200, description = "角色权限ID列表")),
    security(("bearer" = [])))]
async fn get_role_perms(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<Vec<String>>>> {
    let perm_ids = state
        .permission_service
        .perm_repo
        .find_role_perm_ids(&state.db, id)
        .await?;
    let ids: Vec<String> = perm_ids.iter().map(|p| p.to_string()).collect();
    Ok(Json(ApiResponse::success(ids)))
}

/// 分配菜单给角色
#[utoipa::path(put, path = "/api/v1/system/roles/{id}/menus", tag = "角色管理",
    params(("id" = i64, Path)),
    request_body = AssignMenusDto,
    responses((status = 200, description = "菜单分配成功")),
    security(("bearer" = [])))]
async fn assign_menus(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<AssignMenusDto>,
) -> AppResult<Json<ApiResponse<()>>> {
    let menu_ids: Vec<i64> = dto.menu_ids.iter().filter_map(|s| s.parse().ok()).collect();
    state
        .role_service
        .assign_menus(&state.db, id, menu_ids)
        .await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("菜单分配成功")))
}

/// 设置角色数据权限
#[utoipa::path(put, path = "/api/v1/system/roles/{id}/data-scope", tag = "角色管理",
    params(("id" = i64, Path)),
    request_body = AssignDataScopeDto,
    responses((status = 200, description = "数据权限设置成功")),
    security(("bearer" = [])))]
async fn assign_data_scope(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<AssignDataScopeDto>,
) -> AppResult<Json<ApiResponse<()>>> {
    let dept_ids: Vec<i64> = dto.dept_ids.iter().filter_map(|s| s.parse().ok()).collect();
    state
        .role_service
        .assign_data_scope(&state.db, id, &dto.data_scope, dept_ids)
        .await?;
    Ok(Json(ApiResponse::success_no_data_with_msg(
        "数据权限设置成功",
    )))
}
