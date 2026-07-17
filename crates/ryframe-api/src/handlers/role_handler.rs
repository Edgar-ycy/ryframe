use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use ryframe_auth::rbac;
use ryframe_common::{ApiPageResponse, ApiResponse, AppError, AppResult};
use ryframe_core::PageQuery;
use ryframe_macro::{delete, get, post, put, route};
use ryframe_service::system::{RoleListParams, RoleVo};
use serde::Serialize;
use validator::Validate;

use crate::dto::role_dto::{
    CreateRoleDto, ReplaceRoleDataScopeDto, ReplaceRolePermissionsDto, UpdateRoleDto,
};
use crate::handler_utils::{excel_response, parse_csv_i64, parse_i64_strings};
use crate::state::AppState;
use crate::{detail_body, list_query};
use ryframe_auth::RequestPrincipal;

list_query!(pub RoleListQuery, RoleFilterQuery {
    name: String,
    code: String,
    status: String,
});

impl RoleFilterQuery {
    fn into_service_params(self, page: PageQuery) -> RoleListParams {
        RoleListParams {
            page,
            name: self.name,
            code: self.code,
            status: self.status,
        }
    }
}

async fn ensure_can_operate_role(
    state: &AppState,
    current_user: &RequestPrincipal,
    role_id: i64,
) -> AppResult<()> {
    let role = state
        .services
        .role
        .get_role_model(current_user, role_id)
        .await?;
    if role.is_super == 1
        && !current_user.is_super_admin
        && !rbac::has_permission(&current_user.permissions, "sys:role:editSuper")
    {
        return Err(AppError::Authorization("无权限操作超级管理员角色".into()));
    }
    Ok(())
}

pub fn role_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list))
        .merge(route!(list_no_page))
        .merge(route!(export_roles))
        .merge(route!(detail))
        .merge(route!(create))
        .merge(route!(update))
        .merge(route!(remove))
        .merge(route!(batch_remove))
        .merge(route!(get_role_perms))
        .merge(route!(replace_permissions))
        .merge(route!(replace_data_scope))
        .with_state(state)
}

/// 角色列表分页查询
#[get("/")]
#[perm("system:role:list")]
#[utoipa::path(get, path = "/api/v1/system/roles", tag = "角色管理",
    params(RoleListQuery),
    responses((status = 200, description = "角色列表", body = ApiPageResponse<RoleVo>)), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<RoleListQuery>,
) -> AppResult<Json<ApiPageResponse<RoleVo>>> {
    let (page, filter) = query.into_parts();
    state
        .services
        .role
        .find_by_page(&current_user, filter.into_service_params(page))
        .await
        .map(|p| Json(p.to_page_response("查询成功")))
}

/// 角色列表不分页查询（返回全部数据）
#[get("/all")]
#[perm("system:role:list")]
#[utoipa::path(get, path = "/api/v1/system/roles/all", tag = "角色管理",
    params(RoleFilterQuery),
    responses((status = 200, description = "角色列表", body = ApiResponse<Vec<RoleVo>>)),
    security(("bearer" = [])))]
async fn list_no_page(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<RoleFilterQuery>,
) -> AppResult<Json<ApiResponse<Vec<RoleVo>>>> {
    state
        .services
        .role
        .find_by_page(
            &current_user,
            query.into_service_params(PageQuery::all_records()),
        )
        .await
        .map(|p| Json(ApiResponse::success(p.records)))
}

/// 角色详情
#[get("/{id}")]
#[perm("system:role:list")]
#[utoipa::path(get, path = "/api/v1/system/roles/{id}", tag = "角色管理",
    params(("id" = i64, Path)), responses((status = 200, description = "角色详情", body = ApiResponse<RoleVo>)), security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<RoleVo>>> {
    detail_body!(state, current_user, id, role, RoleVo, "角色")
}

/// 创建角色
#[post("/")]
#[perm("system:role:add")]
#[utoipa::path(post, path = "/api/v1/system/roles", tag = "角色管理",
    request_body = CreateRoleDto, responses((status = 200, description = "创建成功", body = ApiResponse<RoleVo>)), security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(dto): Json<CreateRoleDto>,
) -> AppResult<Json<ApiResponse<RoleVo>>> {
    dto.validate()?;
    state
        .services
        .role
        .create(
            &current_user,
            &dto.name,
            &dto.code,
            dto.sort.unwrap_or(0),
            dto.data_scope,
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 更新角色
#[put("/{id}")]
#[perm("system:role:edit")]
#[utoipa::path(put, path = "/api/v1/system/roles/{id}", tag = "角色管理",
    params(("id" = i64, Path)), request_body = UpdateRoleDto,
    responses((status = 200, description = "更新成功", body = ApiResponse<RoleVo>)), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateRoleDto>,
) -> AppResult<Json<ApiResponse<RoleVo>>> {
    dto.validate()?;
    ensure_can_operate_role(&state, &current_user, id).await?;
    let result = state
        .services
        .role
        .update(
            &current_user,
            id,
            &dto.name,
            dto.sort.unwrap_or(0),
            dto.status,
            None,
        )
        .await?;
    Ok(Json(ApiResponse::success(result)))
}

/// 删除角色
#[delete("/{id}")]
#[perm("system:role:remove")]
#[utoipa::path(delete, path = "/api/v1/system/roles/{id}", tag = "角色管理",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功", body = ryframe_common::ApiEmptyResponse)), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    ensure_can_operate_role(&state, &current_user, id).await?;
    state.services.role.delete(&current_user, id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("删除成功")))
}

/// 批量删除角色
#[delete("/batch/{ids}")]
#[perm("system:role:remove")]
#[utoipa::path(delete, path = "/api/v1/system/roles/batch/{ids}", tag = "角色管理",
    params(("ids" = String, Path)),
    responses((status = 200, description = "批量删除成功", body = ryframe_common::ApiEmptyResponse)),
    security(("bearer" = [])))]
async fn batch_remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(ids_str): Path<String>,
) -> AppResult<Json<ApiResponse<()>>> {
    let ids = parse_csv_i64(&ids_str)?;

    if ids.is_empty() {
        return Err(ryframe_common::AppError::Validation(
            "请选择要删除的角色".into(),
        ));
    }

    for id in &ids {
        ensure_can_operate_role(&state, &current_user, *id).await?;
    }

    let count = state.services.role.delete_many(&current_user, &ids).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg(format!(
        "成功删除 {} 个角色",
        count
    ))))
}

/// 导出角色数据为 Excel
#[get("/export")]
#[perm("system:role:export")]
#[utoipa::path(get, path = "/api/v1/system/roles/export", tag = "角色管理",
    params(RoleFilterQuery),
    responses((status = 200, description = "导出角色 Excel", body = Vec<u8>, content_type = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")), security(("bearer" = [])))]
async fn export_roles(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<RoleFilterQuery>,
) -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;

    // 查询所有角色
    let page_result = state
        .services
        .role
        .find_by_page(
            &current_user,
            query.into_service_params(PageQuery::all_records()),
        )
        .await?;

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
    excel_response(bytes, "roles.xlsx")
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

/// Replace all permissions assigned to one role.
#[put("/{id}/permissions")]
#[perm("system:role:edit")]
#[utoipa::path(put, path = "/api/v1/system/roles/{id}/permissions", tag = "角色管理",
    params(("id" = i64, Path)), request_body = ReplaceRolePermissionsDto,
    responses((status = 200, description = "权限分配成功", body = ryframe_common::ApiEmptyResponse)),
    security(("bearer" = [])))]
async fn replace_permissions(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<ReplaceRolePermissionsDto>,
) -> AppResult<Json<ApiResponse<()>>> {
    dto.validate()?;
    ensure_can_operate_role(&state, &current_user, id).await?;
    let perm_ids = parse_i64_strings(&dto.perm_ids)?;
    state
        .services
        .role
        .assign_permissions(&current_user, id, perm_ids)
        .await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("权限分配成功")))
}

/// 查询角色已分配的权限ID列表
#[get("/{id}/permissions")]
#[perm("system:role:list")]
#[utoipa::path(get, path = "/api/v1/system/roles/{id}/permissions", tag = "角色管理",
    params(("id" = i64, Path)),
    responses((status = 200, description = "角色权限ID列表", body = ApiResponse<Vec<String>>)),
    security(("bearer" = [])))]
async fn get_role_perms(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<Vec<String>>>> {
    let perm_ids = state
        .services
        .permission
        .find_role_permission_ids(&current_user, id)
        .await?;
    let ids: Vec<String> = perm_ids.iter().map(|p| p.to_string()).collect();
    Ok(Json(ApiResponse::success(ids)))
}

/// Atomically replace one role's data scope and custom departments.
#[put("/{id}/data-scope")]
#[perm("system:role:edit")]
#[utoipa::path(put, path = "/api/v1/system/roles/{id}/data-scope", tag = "角色管理",
    params(("id" = i64, Path)), request_body = ReplaceRoleDataScopeDto,
    responses((status = 200, description = "数据权限更新成功", body = ryframe_common::ApiEmptyResponse)),
    security(("bearer" = [])))]
async fn replace_data_scope(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<ReplaceRoleDataScopeDto>,
) -> AppResult<Json<ApiResponse<()>>> {
    dto.validate()?;
    ensure_can_operate_role(&state, &current_user, id).await?;
    let dept_ids = parse_i64_strings(&dto.dept_ids)?;
    state
        .services
        .role
        .replace_data_scope(&current_user, id, &dto.data_scope, dept_ids)
        .await?;
    Ok(Json(ApiResponse::success_no_data_with_msg(
        "数据权限更新成功",
    )))
}
