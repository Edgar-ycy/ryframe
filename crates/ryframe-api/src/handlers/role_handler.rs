use crate::http::{ApiPageResponse, ApiResponse, HttpResult};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use ryframe_application::system::RoleListParams;
use ryframe_auth::rbac;
use ryframe_kernel::AppError;
use ryframe_kernel::ValidatedPageQuery;
use ryframe_macro::{delete, get, post, put, route};
use validator::Validate;

use crate::RequestPrincipal;
use crate::dto::public_dto::{ExportJobVo, OptionList, RoleVo};
use crate::dto::role_dto::{
    CreateRoleDto, ReplaceRoleDataScopeDto, ReplaceRolePermissionsDto, RoleOptionQuery,
    UpdateRoleDto,
};
use crate::handler_utils::{parse_csv_i64, parse_i64_strings};
use crate::state::AppState;
use crate::{detail_body, list_query};
use crate::{dto::export_dto::RoleExportRequestDto, handlers::export_handler::request_export};

list_query!(pub RoleListQuery, RoleFilterQuery {
    name: String,
    code: String,
    status: String,
});

impl RoleFilterQuery {
    fn into_service_params(self, page: ValidatedPageQuery) -> RoleListParams {
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
) -> HttpResult<()> {
    let role = state
        .services
        .role
        .get_role_model(current_user, role_id)
        .await?;
    if role.is_super == 1
        && !current_user.is_super_admin
        && !rbac::has_permission(&current_user.permissions, "sys:role:editSuper")
    {
        return Err(AppError::Authorization("无权限操作超级管理员角色".into()).into());
    }
    Ok(())
}

pub fn role_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list))
        .merge(route!(options))
        .merge(route!(request_role_export))
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

/// 查询当前操作者可以分配的角色选项。
#[get("/options")]
#[perm("system:role:list")]
#[utoipa::path(get, path = "/api/v1/system/roles/options", tag = "角色管理",
    params(RoleOptionQuery),
    responses((status = 200, description = "角色选项", body = ApiResponse<OptionList>)),
    security(("bearer" = [])))]
async fn options(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<RoleOptionQuery>,
) -> HttpResult<Json<ApiResponse<OptionList>>> {
    let query = query.resolve(&state.config.pagination)?;
    state
        .services
        .role
        .find_options(
            &current_user,
            query.purpose,
            query.q.as_deref(),
            query.limit,
        )
        .await
        .map_err(crate::http::HttpAppError::from)
        .map(OptionList::from)
        .map(ApiResponse::success)
        .map(Json)
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
) -> HttpResult<Json<ApiPageResponse<RoleVo>>> {
    let (page, filter) = query.into_parts(&state.config.pagination)?;
    state
        .services
        .role
        .find_by_page(&current_user, filter.into_service_params(page))
        .await
        .map_err(crate::http::HttpAppError::from)
        .map(|p| {
            Json(ApiPageResponse::page(
                p.records.into_iter().map(RoleVo::from).collect(),
                p.total,
                p.page,
                p.page_size,
                state.config.pagination.max_page_size,
            ))
        })
}

/// 角色详情
#[get("/{id}")]
#[perm("system:role:list")]
#[utoipa::path(get, path = "/api/v1/system/roles/{id}", tag = "角色管理",
    params(("id" = String, Path)), responses((status = 200, description = "角色详情", body = ApiResponse<RoleVo>)), security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> HttpResult<Json<ApiResponse<RoleVo>>> {
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
) -> HttpResult<Json<ApiResponse<RoleVo>>> {
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
        .map_err(crate::http::HttpAppError::from)
        .map(|value| Json(ApiResponse::success(value.into())))
}

/// 更新角色
#[put("/{id}")]
#[perm("system:role:edit")]
#[utoipa::path(put, path = "/api/v1/system/roles/{id}", tag = "角色管理",
    params(("id" = String, Path)), request_body = UpdateRoleDto,
    responses((status = 200, description = "更新成功", body = ApiResponse<RoleVo>)), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateRoleDto>,
) -> HttpResult<Json<ApiResponse<RoleVo>>> {
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
    Ok(Json(ApiResponse::success(result.into())))
}

/// 删除角色
#[delete("/{id}")]
#[perm("system:role:remove")]
#[utoipa::path(delete, path = "/api/v1/system/roles/{id}", tag = "角色管理",
    params(("id" = String, Path)), responses((status = 200, description = "删除成功", body = crate::http::ApiEmptyResponse)), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> HttpResult<Json<ApiResponse<()>>> {
    ensure_can_operate_role(&state, &current_user, id).await?;
    state.services.role.delete(&current_user, id).await?;
    Ok(Json(ApiResponse::success_no_data()))
}

/// 批量删除角色
#[delete("/batch/{ids}")]
#[perm("system:role:remove")]
#[utoipa::path(delete, path = "/api/v1/system/roles/batch/{ids}", tag = "角色管理",
    params(("ids" = String, Path)),
    responses((status = 200, description = "批量删除成功", body = crate::http::ApiEmptyResponse)),
    security(("bearer" = [])))]
async fn batch_remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(ids_str): Path<String>,
) -> HttpResult<Json<ApiResponse<()>>> {
    let ids = parse_csv_i64(&ids_str)?;

    if ids.is_empty() {
        return Err(AppError::Validation("请选择要删除的角色".into()).into());
    }

    for id in &ids {
        ensure_can_operate_role(&state, &current_user, *id).await?;
    }

    state.services.role.delete_many(&current_user, &ids).await?;
    Ok(Json(ApiResponse::success_no_data()))
}

/// 创建角色异步导出任务。
#[post("/exports")]
#[perm("system:role:export")]
#[utoipa::path(post, path = "/api/v1/system/roles/exports", tag = "角色管理",
    params(("Idempotency-Key" = String, Header, description = "幂等键")), request_body = RoleExportRequestDto,
    responses((status = 202, description = "角色导出任务已创建", body = ApiResponse<ExportJobVo>)), security(("bearer" = [])))]
async fn request_role_export(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    headers: HeaderMap,
    Json(request): Json<RoleExportRequestDto>,
) -> HttpResult<(StatusCode, Json<ApiResponse<ExportJobVo>>)> {
    let (selection, confirm_all) = request.into_selection();
    request_export(
        state,
        current_user,
        headers,
        "system:role:export",
        selection,
        confirm_all,
    )
    .await
}

/// 替换一个角色已分配的全部权限。
#[put("/{id}/permissions")]
#[perm("system:role:edit")]
#[utoipa::path(put, path = "/api/v1/system/roles/{id}/permissions", tag = "角色管理",
    params(("id" = String, Path)), request_body = ReplaceRolePermissionsDto,
    responses((status = 200, description = "权限分配成功", body = crate::http::ApiEmptyResponse)),
    security(("bearer" = [])))]
async fn replace_permissions(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<ReplaceRolePermissionsDto>,
) -> HttpResult<Json<ApiResponse<()>>> {
    dto.validate()?;
    ensure_can_operate_role(&state, &current_user, id).await?;
    let perm_ids = parse_i64_strings(&dto.perm_ids)?;
    state
        .services
        .role
        .assign_permissions(&current_user, id, perm_ids)
        .await?;
    Ok(Json(ApiResponse::success_no_data()))
}

/// 查询角色已分配的权限ID列表
#[get("/{id}/permissions")]
#[perm("system:role:list")]
#[utoipa::path(get, path = "/api/v1/system/roles/{id}/permissions", tag = "角色管理",
    params(("id" = String, Path)),
    responses((status = 200, description = "角色权限ID列表", body = ApiResponse<Vec<String>>)),
    security(("bearer" = [])))]
async fn get_role_perms(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> HttpResult<Json<ApiResponse<Vec<String>>>> {
    let perm_ids = state
        .services
        .permission
        .find_role_permission_ids(&current_user, id)
        .await?;
    let ids: Vec<String> = perm_ids.iter().map(|p| p.to_string()).collect();
    Ok(Json(ApiResponse::success(ids)))
}

/// 原子替换一个角色的数据范围和自定义部门。
#[put("/{id}/data-scope")]
#[perm("system:role:edit")]
#[utoipa::path(put, path = "/api/v1/system/roles/{id}/data-scope", tag = "角色管理",
    params(("id" = String, Path)), request_body = ReplaceRoleDataScopeDto,
    responses((status = 200, description = "数据权限更新成功", body = crate::http::ApiEmptyResponse)),
    security(("bearer" = [])))]
async fn replace_data_scope(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<ReplaceRoleDataScopeDto>,
) -> HttpResult<Json<ApiResponse<()>>> {
    dto.validate()?;
    ensure_can_operate_role(&state, &current_user, id).await?;
    let dept_ids = parse_i64_strings(&dto.dept_ids)?;
    state
        .services
        .role
        .replace_data_scope(&current_user, id, &dto.data_scope, dept_ids)
        .await?;
    Ok(Json(ApiResponse::success_no_data()))
}
