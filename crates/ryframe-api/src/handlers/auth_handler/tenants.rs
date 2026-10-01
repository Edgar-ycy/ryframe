use axum::{
    Extension, Json,
    extract::{ConnectInfo, Query, State},
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use utoipa::{IntoParams, ToSchema};

use crate::{
    http::{ApiResponse, HttpResult},
    state::AppState,
};

#[derive(Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct LoginTenantQuery {
    pub search: Option<String>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

#[derive(Serialize, ToSchema)]
pub struct LoginTenantChoice {
    pub tenant_id: String,
    pub name: String,
}

#[derive(Serialize, ToSchema)]
pub struct LoginTenantPage {
    pub items: Vec<LoginTenantChoice>,
    pub has_more: bool,
}

/// 登录前仅公开启用、未过期租户的名称和标识。
#[utoipa::path(get, path = "/api/v1/auth/tenants", tag = "认证",
    params(LoginTenantQuery),
    responses((status = 200, description = "可登录租户", body = ApiResponse<LoginTenantPage>),
        (status = 400, description = "查询参数无效"), (status = 429, description = "请求过于频繁")))]
pub async fn login_tenants(
    State(state): State<AppState>,
    client_ip: Option<Extension<crate::ClientIp>>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Query(query): Query<LoginTenantQuery>,
) -> HttpResult<Json<ApiResponse<LoginTenantPage>>> {
    if state.settings.rate_limit.enabled {
        let ip = client_ip.map_or_else(|| address.ip(), |Extension(ip)| ip.0);
        let decision = state
            .rate_limiter
            .acquire(&format!("auth:tenants:{ip}"), 60, 60)
            .await
            .map_err(|error| {
                tracing::error!(%error, "租户选项限流服务不可用");
                ryframe_kernel::AppError::ServiceUnavailable("租户选项限流服务暂不可用".into())
            })?;
        if !decision.allowed {
            return Err(ryframe_kernel::AppError::RateLimited(
                "租户查询过于频繁，请稍后重试".into(),
                decision.retry_after_secs,
            )
            .into());
        }
    }
    let only_tenant = state.settings.multi_tenancy.fixed_tenant_id();
    let page = state
        .services
        .platform
        .tenant
        .login_choices(
            query.search.as_deref().unwrap_or_default(),
            query.page.unwrap_or(1),
            query.page_size.unwrap_or(20),
            only_tenant,
        )
        .await?;
    Ok(Json(ApiResponse::success(LoginTenantPage {
        items: page
            .items
            .into_iter()
            .map(|item| LoginTenantChoice {
                tenant_id: item.tenant_id,
                name: item.name,
            })
            .collect(),
        has_more: page.has_more,
    })))
}
