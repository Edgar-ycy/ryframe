use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use ryframe_auth::{RequestPrincipal, rbac};
use ryframe_http::{ApiPageResponse, ApiResponse, HttpResult};
use ryframe_kernel::AppError;
use ryframe_macro::{get, post, put, route};
use ryframe_service::system::{CreateTenantParams, UpdateTenantParams};
use validator::Validate;

use crate::{
    dto::{
        public_dto::{TenantCapacityVo, TenantUsageVo, TenantVo},
        tenant_dto::{
            CreateTenantDto, TenantCapacityPageQuery, UpdateTenantDto, UpdateTenantStatusDto,
        },
    },
    state::AppState,
};

pub fn tenant_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list))
        .merge(route!(page))
        .merge(route!(detail))
        .merge(route!(usage))
        .merge(route!(create))
        .merge(route!(update))
        .merge(route!(update_status))
        .with_state(state)
}

#[get("/page")]
#[perm("tenant:list")]
#[utoipa::path(
    get,
    path = "/api/v1/platform/tenants/page",
    tag = "租户管理",
    params(TenantCapacityPageQuery),
    responses(
        (status = 200, description = "平台租户分页列表", body = ApiPageResponse<TenantCapacityVo>),
        (status = 400, description = "分页或筛选参数无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "不是系统租户、缺少租户列表权限，或没有容量筛选权限")
    ),
    security(("bearer" = []))
)]
async fn page(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<TenantCapacityPageQuery>,
) -> HttpResult<Json<ApiPageResponse<TenantCapacityVo>>> {
    ensure_system_tenant(&current_user)?;
    query.validate()?;
    let include_usage = can_view_usage(&current_user);
    if query.has_capacity_filter() && !include_usage {
        return Err(AppError::Authorization("筛选租户容量状态需要租户用量查看权限".into()).into());
    }
    let (params, query) = query.into_service_params()?;
    let result = state
        .services
        .tenant_usage
        .page(&current_user, params, &query, include_usage)
        .await?;
    Ok(Json(ApiPageResponse::page(
        result.records.into_iter().map(Into::into).collect(),
        result.total,
        result.page,
        result.page_size,
        TenantCapacityPageQuery::max_page_size(),
    )))
}

#[get("/{tenant_id}")]
#[perm("tenant:list")]
#[utoipa::path(
    get,
    path = "/api/v1/platform/tenants/{tenant_id}",
    tag = "租户管理",
    params(("tenant_id" = String, Path, description = "租户标识")),
    responses(
        (status = 200, description = "平台租户详情", body = ApiResponse<TenantCapacityVo>),
        (status = 400, description = "租户标识无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "不是系统租户或缺少租户列表权限"),
        (status = 404, description = "租户不存在")
    ),
    security(("bearer" = []))
)]
async fn detail(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(tenant_id): Path<String>,
) -> HttpResult<Json<ApiResponse<TenantCapacityVo>>> {
    ensure_system_tenant(&current_user)?;
    let tenant = state
        .services
        .tenant_usage
        .detail(&current_user, &tenant_id, can_view_usage(&current_user))
        .await?;
    Ok(Json(ApiResponse::success(tenant.into())))
}

#[get("/{tenant_id}/usage")]
#[perm("tenant:usage:list")]
#[utoipa::path(
    get,
    path = "/api/v1/platform/tenants/{tenant_id}/usage",
    tag = "租户管理",
    params(("tenant_id" = String, Path, description = "租户标识")),
    responses(
        (status = 200, description = "租户容量与当前请求窗口用量", body = ApiResponse<TenantUsageVo>),
        (status = 400, description = "租户标识无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "不是系统租户或缺少租户用量查看权限"),
        (status = 404, description = "租户不存在")
    ),
    security(("bearer" = []))
)]
async fn usage(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(tenant_id): Path<String>,
) -> HttpResult<Json<ApiResponse<TenantUsageVo>>> {
    ensure_system_tenant(&current_user)?;
    let usage = state
        .services
        .tenant_usage
        .usage(&current_user, &tenant_id)
        .await?;
    Ok(Json(ApiResponse::success(usage.into())))
}

fn can_view_usage(current_user: &RequestPrincipal) -> bool {
    current_user.is_super_admin
        || rbac::has_permission(&current_user.permissions, "tenant:usage:list")
}

fn ensure_system_tenant(current_user: &RequestPrincipal) -> HttpResult<()> {
    if current_user.tenant_id != "system" {
        return Err(AppError::Authorization("仅系统租户可以查看平台租户".into()).into());
    }
    Ok(())
}

#[get("/")]
#[perm("tenant:list")]
#[utoipa::path(get, path = "/api/v1/platform/tenants", tag = "租户管理",
    responses((status = 200, description = "租户列表", body = ApiResponse<Vec<TenantVo>>)), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> HttpResult<Json<ApiResponse<Vec<TenantVo>>>> {
    let tenants = state.services.tenant.list(&current_user).await?;
    Ok(Json(ApiResponse::success(
        tenants.into_iter().map(TenantVo::from).collect(),
    )))
}

#[post("/")]
#[perm("tenant:add")]
#[utoipa::path(post, path = "/api/v1/platform/tenants", tag = "租户管理",
    responses((status = 200, description = "租户创建成功", body = ApiResponse<TenantVo>)), security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(dto): Json<CreateTenantDto>,
) -> HttpResult<Json<ApiResponse<TenantVo>>> {
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
    Ok(Json(ApiResponse::success(model.into())))
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
) -> HttpResult<Json<ApiResponse<TenantVo>>> {
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
    Ok(Json(ApiResponse::success(updated.into())))
}

#[put("/{tenant_id}/status")]
#[perm("tenant:status")]
#[utoipa::path(put, path = "/api/v1/platform/tenants/{tenant_id}/status", tag = "租户管理",
    params(("tenant_id" = String, Path)), responses((status = 200, description = "租户状态更新成功", body = ryframe_http::ApiEmptyResponse)),
    security(("bearer" = [])))]
async fn update_status(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(tenant_id): Path<String>,
    Json(dto): Json<UpdateTenantStatusDto>,
) -> HttpResult<Json<ApiResponse<()>>> {
    state
        .services
        .tenant
        .update_status(&current_user, &tenant_id, dto.status)
        .await?;
    Ok(Json(ApiResponse::success_no_data()))
}
