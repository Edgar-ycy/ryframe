use axum::{
    Extension, Json, Router,
    extract::{Path, State},
};
use chrono::{DateTime, Utc};
use ryframe_auth::jwt::Claims;
use ryframe_common::{ApiResponse, AppError, AppResult};
use ryframe_db::entities::tenant;
use ryframe_macro::{get, post, put, route};
use ryframe_service::system::{CreateTenantParams, UpdateTenantParams};
use serde::{Deserialize, Serialize};
use validator::Validate;

use super::auth_handler::AppState;

#[derive(Debug, Deserialize, Validate)]
pub struct CreateTenantDto {
    #[validate(length(min = 2, max = 64))]
    pub tenant_id: String,
    #[validate(length(min = 1, max = 128))]
    pub name: String,
    pub domain: Option<String>,
    pub expire_at: Option<DateTime<Utc>>,
    pub max_users: Option<i32>,
    pub max_roles: Option<i32>,
    pub max_storage_mb: Option<i64>,
    pub max_requests_per_min: Option<i32>,
    #[validate(length(min = 2, max = 64))]
    pub admin_username: String,
    #[validate(length(min = 8, max = 128))]
    pub admin_password: String,
}

#[derive(Debug, Deserialize, Validate)]
pub struct UpdateTenantDto {
    #[validate(length(min = 1, max = 128))]
    pub name: String,
    pub domain: Option<String>,
    pub expire_at: Option<DateTime<Utc>>,
    pub max_users: i32,
    pub max_roles: i32,
    pub max_storage_mb: i64,
    pub max_requests_per_min: i32,
}

#[derive(Debug, Serialize)]
pub struct TenantVo {
    pub tenant_id: String,
    pub name: String,
    pub domain: Option<String>,
    pub status: String,
    pub expire_at: Option<DateTime<Utc>>,
    pub max_users: i32,
    pub max_roles: i32,
    pub max_storage_mb: i64,
    pub max_requests_per_min: i32,
}

impl From<tenant::Model> for TenantVo {
    fn from(value: tenant::Model) -> Self {
        Self {
            tenant_id: value.tenant_id,
            name: value.name,
            domain: value.domain,
            status: value.status,
            expire_at: value.expire_at,
            max_users: value.max_users,
            max_roles: value.max_roles,
            max_storage_mb: value.max_storage_mb,
            max_requests_per_min: value.max_requests_per_min,
        }
    }
}

fn platform_admin(claims: &Claims) -> AppResult<()> {
    if claims.tenant_id != "system" {
        return Err(AppError::Authorization(
            "only system tenant can manage tenants".into(),
        ));
    }
    Ok(())
}

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
async fn list(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> AppResult<Json<ApiResponse<Vec<TenantVo>>>> {
    platform_admin(&claims)?;
    let tenants = state.tenant_service.list(&state.db).await?;
    Ok(Json(ApiResponse::success(
        tenants.into_iter().map(TenantVo::from).collect(),
    )))
}

#[post("/")]
#[perm("tenant:add")]
async fn create(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(dto): Json<CreateTenantDto>,
) -> AppResult<Json<ApiResponse<TenantVo>>> {
    platform_admin(&claims)?;
    dto.validate()?;
    let model = state
        .tenant_service
        .create(
            &state.db,
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
    state.tenant_rate_limit_cache.invalidate(&model.tenant_id);
    Ok(Json(ApiResponse::success(model.into())))
}

#[put("/{tenant_id}")]
#[perm("tenant:edit")]
async fn update(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(tenant_id): Path<String>,
    Json(dto): Json<UpdateTenantDto>,
) -> AppResult<Json<ApiResponse<TenantVo>>> {
    platform_admin(&claims)?;
    dto.validate()?;
    let updated = state
        .tenant_service
        .update(
            &state.db,
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
    state.tenant_rate_limit_cache.invalidate(&tenant_id);
    Ok(Json(ApiResponse::success(updated.into())))
}

#[derive(Deserialize)]
struct StatusDto {
    status: String,
}

#[put("/{tenant_id}/status")]
#[perm("tenant:status")]
async fn update_status(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(tenant_id): Path<String>,
    Json(dto): Json<StatusDto>,
) -> AppResult<Json<ApiResponse<()>>> {
    platform_admin(&claims)?;
    state
        .tenant_service
        .update_status(&state.db, &tenant_id, dto.status)
        .await?;
    state.tenant_rate_limit_cache.invalidate(&tenant_id);
    Ok(Json(ApiResponse::success_no_data()))
}
