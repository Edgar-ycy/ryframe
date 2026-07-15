use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
};
use ryframe_auth::rbac;
use ryframe_common::{ApiPageResponse, ApiResponse, AppError, AppResult};
use ryframe_core::PageQuery;
use ryframe_macro::{delete, get, post, put, route};
use ryframe_service::system::RoleVo;
use serde::Serialize;
use validator::Validate;

use super::auth_handler::AppState;
use crate::dto::role_dto::{
    CreateRoleDto, RoleDataScopeUpdateDto, RoleDeptAssignDto, RolePermAssignDto, UpdateRoleDto,
};
use crate::extractors::CurrentUser;
use crate::handler_utils::{excel_response, parse_csv_i64, parse_i64_strings};
use crate::{detail_body, list_query};

list_query!(pub RoleListQuery {
    name: String,
    code: String,
    status: String,
});

async fn ensure_can_operate_role(
    state: &AppState,
    current_user: &CurrentUser,
    role_id: i64,
) -> AppResult<()> {
    let role = state
        .role_service
        .get_role_model(&state.db, role_id)
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
        .with_state(state)
}

pub fn role_assignment_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(assign_perm))
        .merge(route!(assign_dept))
        .merge(route!(update_data_scope))
        .with_state(state)
}

/// 角色列表分页查询
#[get("/", "/list")]
#[perm("system:role:list")]
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
#[get("/listNoPage")]
#[perm("system:role:list")]
#[utoipa::path(get, path = "/api/v1/system/roles/listNoPage", tag = "角色管理",
    responses((status = 200, description = "角色列表")),
    security(("bearer" = [])))]
async fn list_no_page(State(state): State<AppState>) -> AppResult<Json<ApiResponse<Vec<RoleVo>>>> {
    let page_query = PageQuery::all_records();
    state
        .role_service
        .find_by_page(&state.db, page_query)
        .await
        .map(|p| Json(ApiResponse::success(p.records)))
}

/// 角色详情
#[get("/{id}")]
#[perm("system:role:list")]
#[utoipa::path(get, path = "/api/v1/system/roles/{id}", tag = "角色管理",
    params(("id" = i64, Path)), responses((status = 200, description = "角色详情")), security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<RoleVo>>> {
    detail_body!(state, id, role_service, RoleVo, "角色")
}

/// 创建角色
#[post("/")]
#[perm("system:role:add")]
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
#[put("/{id}")]
#[perm("system:role:edit")]
#[utoipa::path(put, path = "/api/v1/system/roles/{id}", tag = "角色管理",
    params(("id" = i64, Path)), request_body = UpdateRoleDto,
    responses((status = 200, description = "更新成功")), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    Extension(current_user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateRoleDto>,
) -> AppResult<Json<ApiResponse<RoleVo>>> {
    dto.validate()?;
    ensure_can_operate_role(&state, &current_user, id).await?;
    let result = state
        .role_service
        .update(
            &state.db,
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
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功")), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    Extension(current_user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    ensure_can_operate_role(&state, &current_user, id).await?;
    state.role_service.delete(&state.db, id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("删除成功")))
}

/// 批量删除角色
#[delete("/batch/{ids}")]
#[perm("system:role:remove")]
#[utoipa::path(delete, path = "/api/v1/system/roles/batch/{ids}", tag = "角色管理",
    params(("ids" = String, Path)),
    responses((status = 200, description = "批量删除成功")),
    security(("bearer" = [])))]
async fn batch_remove(
    State(state): State<AppState>,
    Extension(current_user): Extension<CurrentUser>,
    Path(ids_str): Path<String>,
) -> AppResult<Json<ApiResponse<()>>> {
    let ids = parse_csv_i64(&ids_str);

    if ids.is_empty() {
        return Err(ryframe_common::AppError::Validation(
            "请选择要删除的角色".into(),
        ));
    }

    for id in &ids {
        ensure_can_operate_role(&state, &current_user, *id).await?;
    }

    let count = state.role_service.delete_many(&state.db, &ids).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg(format!(
        "成功删除 {} 个角色",
        count
    ))))
}

/// 导出角色数据为 Excel
#[get("/export")]
#[perm("system:role:export")]
async fn export_roles(State(state): State<AppState>) -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;

    // 查询所有角色
    let query = PageQuery::all_records();
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

/// 分配权限给角色
#[post("/assign-perm")]
#[perm("system:role:edit")]
#[utoipa::path(post, path = "/api/v1/system/role/assign-perm", tag = "角色管理",
    request_body = RolePermAssignDto,
    responses((status = 200, description = "权限分配成功")),
    security(("bearer" = [])))]
async fn assign_perm(
    State(state): State<AppState>,
    Extension(current_user): Extension<CurrentUser>,
    Json(dto): Json<RolePermAssignDto>,
) -> AppResult<Json<ApiResponse<()>>> {
    dto.validate()?;
    let role_id = dto
        .role_id
        .parse::<i64>()
        .map_err(|_| ryframe_common::AppError::Validation("无效的角色ID".into()))?;
    ensure_can_operate_role(&state, &current_user, role_id).await?;
    let perm_ids = parse_i64_strings(&dto.perm_ids);
    state
        .role_service
        .assign_permissions(&state.db, role_id, perm_ids)
        .await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("权限分配成功")))
}

/// 查询角色已分配的权限ID列表
#[get("/{id}/permissions")]
#[perm("system:role:list")]
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

/// 分配角色自定义数据权限部门
#[post("/assign-dept")]
#[perm("system:role:edit")]
#[utoipa::path(post, path = "/api/v1/system/role/assign-dept", tag = "角色管理",
    request_body = RoleDeptAssignDto,
    responses((status = 200, description = "自定义部门分配成功")),
    security(("bearer" = [])))]
async fn assign_dept(
    State(state): State<AppState>,
    Extension(current_user): Extension<CurrentUser>,
    Json(dto): Json<RoleDeptAssignDto>,
) -> AppResult<Json<ApiResponse<()>>> {
    dto.validate()?;
    let role_id = dto
        .role_id
        .parse::<i64>()
        .map_err(|_| ryframe_common::AppError::Validation("无效的角色ID".into()))?;
    ensure_can_operate_role(&state, &current_user, role_id).await?;
    let dept_ids = parse_i64_strings(&dto.dept_ids);
    state
        .role_service
        .assign_depts(&state.db, role_id, dept_ids)
        .await?;
    Ok(Json(ApiResponse::success_no_data_with_msg(
        "自定义部门分配成功",
    )))
}

/// 更新角色数据权限范围
#[post("/update-data-scope")]
#[perm("system:role:edit")]
#[utoipa::path(post, path = "/api/v1/system/role/update-data-scope", tag = "角色管理",
    request_body = RoleDataScopeUpdateDto,
    responses((status = 200, description = "数据权限范围更新成功")),
    security(("bearer" = [])))]
async fn update_data_scope(
    State(state): State<AppState>,
    Extension(current_user): Extension<CurrentUser>,
    Json(dto): Json<RoleDataScopeUpdateDto>,
) -> AppResult<Json<ApiResponse<()>>> {
    dto.validate()?;
    let role_id = dto
        .role_id
        .parse::<i64>()
        .map_err(|_| ryframe_common::AppError::Validation("无效的角色ID".into()))?;
    ensure_can_operate_role(&state, &current_user, role_id).await?;
    state
        .role_service
        .update_data_scope(&state.db, role_id, &dto.data_scope)
        .await?;
    Ok(Json(ApiResponse::success_no_data_with_msg(
        "数据权限范围更新成功",
    )))
}
