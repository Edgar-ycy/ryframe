use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use ryframe_application::system::{
    CreateDelegationCommand, ServiceAccountService, ServiceDelegationTargetVo,
};
use ryframe_auth::RequestPrincipal;
use ryframe_http::{ApiResponse, HttpResult};
use ryframe_kernel::AppError;
use ryframe_macro::{delete, get, post, route};
use validator::Validate;

use crate::{
    dto::{
        public_dto::{CreatedServiceDelegationVo, ServiceDelegationVo},
        service_account_dto::CreateServiceDelegationDto,
    },
    handler_utils::idempotency_key_value,
    state::AppState,
};

pub fn service_delegation_profile_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list_my_delegations))
        .merge(route!(delegation_capabilities))
        .merge(route!(create_my_delegation))
        .merge(route!(revoke_my_delegation))
        .with_state(state)
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct ServiceDelegationTargetResponse {
    pub account_id: String,
    pub account_code: String,
    pub account_name: String,
    pub capabilities: Vec<crate::dto::public_dto::ServiceCapabilityVo>,
}

impl From<ServiceDelegationTargetVo> for ServiceDelegationTargetResponse {
    fn from(value: ServiceDelegationTargetVo) -> Self {
        Self {
            account_id: value.account_id,
            account_code: value.code,
            account_name: value.name,
            capabilities: value.capabilities.into_iter().map(Into::into).collect(),
        }
    }
}

#[get("/")]
#[capability("system.service_accounts")]
#[utoipa::path(get, path = "/api/v1/profile/service-delegations", tag = "个人服务委托",
    responses(
        (status = 200, description = "当前用户本人创建的委托", body = ApiResponse<Vec<ServiceDelegationVo>>),
        (status = 401, description = "未认证"),
        (status = 503, description = "服务账号功能未启用或数据库不可用")
    ), security(("bearer" = [])))]
async fn list_my_delegations(
    State(state): State<AppState>,
    actor: RequestPrincipal,
) -> HttpResult<Json<ApiResponse<Vec<ServiceDelegationVo>>>> {
    let values = service(&state)?.list_my_delegations(&actor).await?;
    Ok(Json(ApiResponse::success(
        values.into_iter().map(Into::into).collect(),
    )))
}

#[get("/capabilities")]
#[capability("system.service_accounts")]
#[utoipa::path(get, path = "/api/v1/profile/service-delegations/capabilities", tag = "个人服务委托",
    responses(
        (status = 200, description = "当前用户与服务账号共同可委托的编译期能力", body = ApiResponse<Vec<ServiceDelegationTargetResponse>>),
        (status = 401, description = "未认证"),
        (status = 503, description = "服务账号功能未启用或数据库不可用")
    ), security(("bearer" = [])))]
async fn delegation_capabilities(
    State(state): State<AppState>,
    actor: RequestPrincipal,
) -> HttpResult<Json<ApiResponse<Vec<ServiceDelegationTargetResponse>>>> {
    let values = service(&state)?.delegation_targets(&actor).await?;
    Ok(Json(ApiResponse::success(
        values.into_iter().map(Into::into).collect(),
    )))
}

#[post("/")]
#[capability("system.service_accounts")]
#[utoipa::path(post, path = "/api/v1/profile/service-delegations", tag = "个人服务委托",
    params(("Idempotency-Key" = String, Header)), request_body = CreateServiceDelegationDto,
    responses(
        (status = 200, description = "委托已创建；令牌只显示一次", body = ApiResponse<CreatedServiceDelegationVo>),
        (status = 400, description = "参数、能力或幂等键无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "能力不是双方共同拥有或账号不可委托"),
        (status = 404, description = "服务账号不存在"),
        (status = 409, description = "幂等键冲突"),
        (status = 503, description = "服务账号功能、Pepper 或数据库不可用")
    ), security(("bearer" = [])))]
async fn create_my_delegation(
    State(state): State<AppState>,
    actor: RequestPrincipal,
    headers: HeaderMap,
    Json(request): Json<CreateServiceDelegationDto>,
) -> HttpResult<Response> {
    request.validate()?;
    let value: CreatedServiceDelegationVo = service(&state)?
        .create_delegation(
            &actor,
            CreateDelegationCommand {
                account_id: parse_positive_id(&request.service_account_id)?,
                capability_keys: request.capability_keys,
                expires_at: request.expires_at,
                reason: request.reason,
                idempotency_key: idempotency_key_value(&headers)?,
            },
        )
        .await?
        .into();
    Ok(one_time_response(value))
}

#[delete("/{id}")]
#[capability("system.service_accounts")]
#[utoipa::path(delete, path = "/api/v1/profile/service-delegations/{id}", tag = "个人服务委托",
    params(("id" = String, Path)),
    responses(
        (status = 200, description = "本人委托已撤销", body = ryframe_http::ApiEmptyResponse),
        (status = 400, description = "委托 ID 无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "只能撤销本人委托"),
        (status = 404, description = "委托不存在"),
        (status = 409, description = "委托已撤销"),
        (status = 503, description = "服务账号功能未启用或数据库不可用")
    ), security(("bearer" = [])))]
async fn revoke_my_delegation(
    State(state): State<AppState>,
    actor: RequestPrincipal,
    Path(id): Path<String>,
) -> HttpResult<Json<ApiResponse<()>>> {
    service(&state)?
        .revoke_my_delegation(&actor, parse_positive_id(&id)?)
        .await?;
    Ok(Json(ApiResponse::success_no_data()))
}

fn service(state: &AppState) -> HttpResult<&ServiceAccountService> {
    state
        .services
        .service_accounts
        .as_deref()
        .ok_or_else(|| AppError::ServiceUnavailable("服务账号功能未启用".into()).into())
}

fn parse_positive_id(value: &str) -> HttpResult<i64> {
    value
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| AppError::Validation("ID 无效".into()).into())
}

fn one_time_response<T: serde::Serialize>(value: T) -> Response {
    let mut response = Json(ApiResponse::success(value)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}
