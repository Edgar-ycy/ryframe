use axum::{
    Json, Router,
    extract::{Path, State},
};
use ryframe_auth::RequestPrincipal;
use ryframe_common::{ApiResponse, AppResult};
use ryframe_macro::{get, post, put, route};
use ryframe_service::system::{CreateTenantParams, TenantVo, UpdateTenantParams};
use validator::Validate;

use crate::{
    dto::tenant_dto::{CreateTenantDto, UpdateTenantDto, UpdateTenantStatusDto},
    state::AppState,
};

pub fn tenant_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list))
        .merge(route!(create))
        .merge(route!(update))
        .merge(route!(update_status))
        .with_state(state)
}

#[get("/")]
#[perm("tenant:list")]
#[utoipa::path(get, path = "/api/v1/platform/tenants", tag = "租户管理",
    responses((status = 200, description = "租户列表", body = ApiResponse<Vec<TenantVo>>)), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> AppResult<Json<ApiResponse<Vec<TenantVo>>>> {
    let tenants = state.services.tenant.list(&current_user).await?;
    Ok(Json(ApiResponse::success(tenants)))
}

#[post("/")]
#[perm("tenant:add")]
#[utoipa::path(post, path = "/api/v1/platform/tenants", tag = "租户管理",
    responses((status = 200, description = "租户创建成功", body = ApiResponse<TenantVo>)), security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(dto): Json<CreateTenantDto>,
) -> AppResult<Json<ApiResponse<TenantVo>>> {
    dto.validate()?;
    let model = state
        .services
        .tenant
        .create(
            &current_user,
            CreateTenantParams {
                tenant_id: dto.tenant_id,
                name: dto.name,
                domain: dto.domain,
                expire_at: dto.expire_at,
                max_users: dto.max_users,
                max_roles: dto.max_roles,
                max_storage_mb: dto.max_storage_mb,
                max_requests_per_min: dto.max_requests_per_min,
                admin_username: dto.admin_username,
                admin_password: dto.admin_password,
            },
        )
        .await?;
    Ok(Json(ApiResponse::success(model)))
}

#[put("/{tenant_id}")]
#[perm("tenant:edit")]
#[utoipa::path(put, path = "/api/v1/platform/tenants/{tenant_id}", tag = "租户管理",
    params(("tenant_id" = String, Path)), responses((status = 200, description = "租户更新成功", body = ApiResponse<TenantVo>)),
    security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(tenant_id): Path<String>,
    Json(dto): Json<UpdateTenantDto>,
) -> AppResult<Json<ApiResponse<TenantVo>>> {
    dto.validate()?;
    let updated = state
        .services
        .tenant
        .update(
            &current_user,
            &tenant_id,
            UpdateTenantParams {
                name: dto.name,
                domain: dto.domain,
                expire_at: dto.expire_at,
                max_users: dto.max_users,
                max_roles: dto.max_roles,
                max_storage_mb: dto.max_storage_mb,
                max_requests_per_min: dto.max_requests_per_min,
            },
        )
        .await?;
    Ok(Json(ApiResponse::success(updated)))
}

#[put("/{tenant_id}/status")]
#[perm("tenant:status")]
#[utoipa::path(put, path = "/api/v1/platform/tenants/{tenant_id}/status", tag = "租户管理",
    params(("tenant_id" = String, Path)), responses((status = 200, description = "租户状态更新成功", body = ryframe_common::ApiEmptyResponse)),
    security(("bearer" = [])))]
async fn update_status(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(tenant_id): Path<String>,
    Json(dto): Json<UpdateTenantStatusDto>,
) -> AppResult<Json<ApiResponse<()>>> {
    state
        .services
        .tenant
        .update_status(&current_user, &tenant_id, dto.status)
        .await?;
    Ok(Json(ApiResponse::success_no_data()))
}
