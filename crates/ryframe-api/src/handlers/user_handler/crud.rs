use axum::{
    Json,
    extract::{Path, Query, State},
};
use ryframe_auth::RequestPrincipal;
use ryframe_common::{ApiPageResponse, ApiResponse, AppError, AppResult};
use ryframe_core::PageQuery;
use ryframe_macro::{delete, get, post, put};
use ryframe_service::system::{
    CreateUserParams, UpdateUserParams, UserDetailVo, UserListParams, UserVo,
};
use validator::Validate;

use super::{UserListQuery, ensure_current_user_permission};
use crate::{
    dto::user_dto::{CreateUserDto, ReplaceUserRolesDto, UpdateUserDto, UpdateUserStatusDto},
    handler_utils::{parse_csv_i64, parse_i64_strings, parse_optional_i64},
    state::AppState,
};

#[get("/")]
#[perm("system:user:list")]
#[utoipa::path(get, path = "/api/v1/system/users", tag = "用户管理",
    params(UserListQuery),
    responses((status = 200, description = "用户列表", body = ApiPageResponse<UserVo>)),
    security(("bearer" = [])))]
pub(crate) async fn list(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<UserListQuery>,
) -> AppResult<Json<ApiPageResponse<UserVo>>> {
    let params = query.into_service_params()?;
    state
        .services
        .user
        .find_by_page(&current_user, params)
        .await
        .map(|page| Json(page.to_page_response("查询成功")))
}

#[get("/all")]
#[perm("system:user:list")]
#[utoipa::path(get, path = "/api/v1/system/users/all", tag = "用户管理",
    responses((status = 200, description = "用户列表", body = ApiResponse<Vec<UserVo>>)),
    security(("bearer" = [])))]
pub(crate) async fn list_no_page(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> AppResult<Json<ApiResponse<Vec<UserVo>>>> {
    state
        .services
        .user
        .find_by_page(
            &current_user,
            UserListParams::page_only(PageQuery::all_records()),
        )
        .await
        .map(|page| Json(ApiResponse::success(page.records)))
}

#[get("/{id}")]
#[perm("system:user:list")]
#[utoipa::path(get, path = "/api/v1/system/users/{id}", tag = "用户管理",
    params(("id" = i64, Path, description = "用户ID")),
    responses((status = 200, description = "用户详情", body = ApiResponse<UserDetailVo>)),
    security(("bearer" = [])))]
pub(crate) async fn detail(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<UserDetailVo>>> {
    state
        .services
        .user
        .find_by_id(&current_user, id)
        .await?
        .map(|user| Json(ApiResponse::success(user)))
        .ok_or_else(|| AppError::NotFound("用户不存在".into()))
}

#[post("/")]
#[perm("system:user:add")]
#[utoipa::path(post, path = "/api/v1/system/users", tag = "用户管理",
    request_body = CreateUserDto,
    responses((status = 200, description = "创建成功", body = ApiResponse<UserVo>)),
    security(("bearer" = [])))]
pub(crate) async fn create(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(dto): Json<CreateUserDto>,
) -> AppResult<Json<ApiResponse<UserVo>>> {
    dto.validate()?;
    let dept_id = parse_optional_i64(dto.dept_id)?;
    let role_ids = parse_i64_strings(&dto.role_ids)?;
    state
        .services
        .user
        .create(
            &current_user,
            CreateUserParams {
                username: &dto.username,
                nickname: &dto.nickname,
                email: dto.email.as_deref().unwrap_or(""),
                phone: dto.phone.as_deref().unwrap_or(""),
                dept_id,
                role_ids,
            },
        )
        .await
        .map(|user| Json(ApiResponse::success(user)))
}

#[put("/{id}")]
#[perm("system:user:edit")]
#[utoipa::path(put, path = "/api/v1/system/users/{id}", tag = "用户管理",
    params(("id" = i64, Path, description = "用户ID")),
    request_body = UpdateUserDto,
    responses((status = 200, description = "更新成功", body = ApiResponse<UserVo>)),
    security(("bearer" = [])))]
pub(crate) async fn update(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateUserDto>,
) -> AppResult<Json<ApiResponse<UserVo>>> {
    dto.validate()?;
    let dept_id = parse_optional_i64(dto.dept_id)?;
    state
        .services
        .user
        .update(
            &current_user,
            UpdateUserParams {
                id,
                nickname: &dto.nickname,
                email: dto.email.as_deref().unwrap_or(""),
                phone: dto.phone.as_deref().unwrap_or(""),
                dept_id,
            },
        )
        .await
        .map(|user| Json(ApiResponse::success(user)))
}

#[put("/{id}/roles")]
#[perm("system:user:edit")]
#[utoipa::path(put, path = "/api/v1/system/users/{id}/roles", tag = "用户管理",
    params(("id" = i64, Path, description = "用户ID")),
    request_body = ReplaceUserRolesDto,
    responses((status = 200, description = "角色分配成功", body = ryframe_common::ApiEmptyResponse)),
    security(("bearer" = [])))]
pub(crate) async fn replace_roles(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<ReplaceUserRolesDto>,
) -> AppResult<Json<ApiResponse<()>>> {
    dto.validate()?;
    let role_ids = parse_i64_strings(&dto.role_ids)?;
    if id == current_user.user_id {
        ensure_current_user_permission(
            &current_user,
            "sys:user:editSelf",
            "不允许修改自身角色权限",
        )?;
    }

    let super_role = state.services.role.get_super_role(&current_user).await?;
    if role_ids.contains(&super_role.id) {
        ensure_current_user_permission(
            &current_user,
            "sys:role:editSuper",
            "无权限修改超级管理员角色",
        )?;
    }
    state
        .services
        .user
        .replace_roles(&current_user, id, role_ids)
        .await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("角色分配成功")))
}

#[delete("/{id}")]
#[perm("system:user:remove")]
#[utoipa::path(delete, path = "/api/v1/system/users/{id}", tag = "用户管理",
    params(("id" = i64, Path, description = "用户ID")),
    responses((status = 200, description = "删除成功", body = ryframe_common::ApiEmptyResponse)),
    security(("bearer" = [])))]
pub(crate) async fn remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    state.services.user.delete(&current_user, id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("删除成功")))
}

#[delete("/batch/{ids}")]
#[perm("system:user:remove")]
#[utoipa::path(delete, path = "/api/v1/system/users/batch/{ids}", tag = "用户管理",
    params(("ids" = String, Path, description = "用户ID列表，逗号分隔")),
    responses((status = 200, description = "批量删除成功", body = ryframe_common::ApiEmptyResponse)),
    security(("bearer" = [])))]
pub(crate) async fn batch_remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(ids): Path<String>,
) -> AppResult<Json<ApiResponse<()>>> {
    let ids = parse_csv_i64(&ids)?;
    if ids.is_empty() {
        return Err(AppError::Validation("请选择要删除的用户".into()));
    }
    let count = state.services.user.delete_many(&current_user, &ids).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg(format!(
        "成功删除 {count} 个用户"
    ))))
}

#[put("/{id}/status")]
#[perm("system:user:edit")]
#[utoipa::path(put, path = "/api/v1/system/users/{id}/status", tag = "用户管理",
    params(("id" = i64, Path, description = "用户ID")),
    request_body = UpdateUserStatusDto,
    responses((status = 200, description = "状态修改成功", body = ryframe_common::ApiEmptyResponse)),
    security(("bearer" = [])))]
pub(crate) async fn update_status(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateUserStatusDto>,
) -> AppResult<Json<ApiResponse<()>>> {
    dto.validate()?;
    state
        .services
        .user
        .update_status(&current_user, id, dto.status)
        .await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("状态修改成功")))
}
