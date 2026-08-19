use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use ryframe_application::system::{
    CreateCredentialCommand, CreateServiceAccountCommand, ServiceAccountService,
    UpdateServiceAccountCommand,
};
use ryframe_auth::RequestPrincipal;
use ryframe_http::{ApiPageResponse, ApiResponse, HttpResult};
use ryframe_kernel::AppError;
use ryframe_macro::{delete, get, post, put, route};
use validator::Validate;

use crate::{
    dto::{
        public_dto::{
            CreatedServiceCredentialVo, ServiceAccessAuditVo, ServiceAccountDetailVo,
            ServiceAccountVo, ServiceCredentialVo, ServiceDelegationVo,
        },
        service_account_dto::{
            CreateServiceAccountDto, CreateServiceCredentialDto, ReplaceServiceAccountRolesDto,
            ServiceResourcePageQuery, UpdateServiceAccountDto, UpdateServiceAccountStatusDto,
        },
    },
    handler_utils::{idempotency_key_value, parse_i64_strings},
    state::AppState,
};

pub fn service_account_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list_accounts))
        .merge(route!(create_account))
        .merge(route!(account_detail))
        .merge(route!(update_account))
        .merge(route!(update_account_status))
        .merge(route!(remove_account))
        .merge(route!(account_roles))
        .merge(route!(replace_account_roles))
        .merge(route!(list_credentials))
        .merge(route!(create_credential))
        .merge(route!(revoke_credential))
        .with_state(state)
}

pub fn service_delegation_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list_delegations))
        .merge(route!(revoke_delegation))
        .with_state(state)
}

pub fn service_access_audit_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list_access_audits))
        .with_state(state)
}

#[get("/")]
#[capability("system.service_accounts")]
#[perm("system:service-account:list")]
#[utoipa::path(get, path = "/api/v1/system/service-accounts", tag = "服务账号",
    params(ServiceResourcePageQuery),
    responses(
        (status = 200, description = "服务账号分页列表", body = ApiPageResponse<ServiceAccountVo>),
        (status = 400, description = "分页参数无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "没有服务账号查看权限"),
        (status = 503, description = "服务账号功能未启用或数据库不可用")
    ), security(("bearer" = [])))]
async fn list_accounts(
    State(state): State<AppState>,
    actor: RequestPrincipal,
    Query(query): Query<ServiceResourcePageQuery>,
) -> HttpResult<Json<ApiPageResponse<ServiceAccountVo>>> {
    let page = service(&state)?
        .list_accounts(&actor, query.into_page(&state.config.pagination)?)
        .await?;
    Ok(Json(ApiPageResponse::page(
        page.records.into_iter().map(Into::into).collect(),
        page.total,
        page.page,
        page.page_size,
        state.config.pagination.max_page_size,
    )))
}

#[post("/")]
#[capability("system.service_accounts")]
#[perm("system:service-account:add")]
#[utoipa::path(post, path = "/api/v1/system/service-accounts", tag = "服务账号",
    request_body = CreateServiceAccountDto,
    responses(
        (status = 200, description = "服务账号已创建", body = ApiResponse<ServiceAccountVo>),
        (status = 400, description = "请求参数无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "没有服务账号创建权限"),
        (status = 409, description = "账号代码冲突"),
        (status = 503, description = "服务账号功能未启用或数据库不可用")
    ), security(("bearer" = [])))]
async fn create_account(
    State(state): State<AppState>,
    actor: RequestPrincipal,
    Json(request): Json<CreateServiceAccountDto>,
) -> HttpResult<Json<ApiResponse<ServiceAccountVo>>> {
    request.validate()?;
    let account = service(&state)?
        .create_account(
            &actor,
            CreateServiceAccountCommand {
                code: request.code,
                name: request.name,
                description: request.description,
                dept_id: parse_optional_positive_id(request.dept_id.as_deref(), "部门")?,
                max_requests_per_minute: request.max_requests_per_minute,
            },
        )
        .await?;
    Ok(Json(ApiResponse::success(account.into())))
}

#[get("/{id}")]
#[capability("system.service_accounts")]
#[perm("system:service-account:list")]
#[utoipa::path(get, path = "/api/v1/system/service-accounts/{id}", tag = "服务账号",
    params(("id" = String, Path)),
    responses(
        (status = 200, description = "服务账号详情", body = ApiResponse<ServiceAccountDetailVo>),
        (status = 400, description = "账号 ID 无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "没有服务账号查看权限"),
        (status = 404, description = "服务账号不存在"),
        (status = 503, description = "服务账号功能未启用或数据库不可用")
    ), security(("bearer" = [])))]
async fn account_detail(
    State(state): State<AppState>,
    actor: RequestPrincipal,
    Path(id): Path<String>,
) -> HttpResult<Json<ApiResponse<ServiceAccountDetailVo>>> {
    let account = service(&state)?
        .account_detail(&actor, parse_positive_id(&id, "服务账号")?)
        .await?;
    Ok(Json(ApiResponse::success(account.into())))
}

#[put("/{id}")]
#[capability("system.service_accounts")]
#[perm("system:service-account:edit")]
#[utoipa::path(put, path = "/api/v1/system/service-accounts/{id}", tag = "服务账号",
    params(("id" = String, Path)), request_body = UpdateServiceAccountDto,
    responses(
        (status = 200, description = "服务账号已更新", body = ApiResponse<ServiceAccountVo>),
        (status = 400, description = "请求参数无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "没有服务账号编辑权限"),
        (status = 404, description = "服务账号不存在"),
        (status = 503, description = "服务账号功能未启用或数据库不可用")
    ), security(("bearer" = [])))]
async fn update_account(
    State(state): State<AppState>,
    actor: RequestPrincipal,
    Path(id): Path<String>,
    Json(request): Json<UpdateServiceAccountDto>,
) -> HttpResult<Json<ApiResponse<ServiceAccountVo>>> {
    request.validate()?;
    let account = service(&state)?
        .update_account(
            &actor,
            parse_positive_id(&id, "服务账号")?,
            UpdateServiceAccountCommand {
                name: request.name,
                description: request.description,
                dept_id: parse_optional_positive_id(request.dept_id.as_deref(), "部门")?,
                max_requests_per_minute: request.max_requests_per_minute,
            },
        )
        .await?;
    Ok(Json(ApiResponse::success(account.into())))
}

#[put("/{id}/status")]
#[capability("system.service_accounts")]
#[perm("system:service-account:edit")]
#[utoipa::path(put, path = "/api/v1/system/service-accounts/{id}/status", tag = "服务账号",
    params(("id" = String, Path)), request_body = UpdateServiceAccountStatusDto,
    responses(
        (status = 200, description = "服务账号状态已更新", body = ryframe_http::ApiEmptyResponse),
        (status = 400, description = "请求参数无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "没有服务账号编辑权限"),
        (status = 404, description = "服务账号不存在"),
        (status = 503, description = "服务账号功能未启用或数据库不可用")
    ), security(("bearer" = [])))]
async fn update_account_status(
    State(state): State<AppState>,
    actor: RequestPrincipal,
    Path(id): Path<String>,
    Json(request): Json<UpdateServiceAccountStatusDto>,
) -> HttpResult<Json<ApiResponse<()>>> {
    service(&state)?
        .update_account_status(
            &actor,
            parse_positive_id(&id, "服务账号")?,
            request.status.as_storage_value().to_owned(),
        )
        .await?;
    Ok(Json(ApiResponse::success_no_data()))
}

#[delete("/{id}")]
#[capability("system.service_accounts")]
#[perm("system:service-account:remove")]
#[utoipa::path(delete, path = "/api/v1/system/service-accounts/{id}", tag = "服务账号",
    params(("id" = String, Path)),
    responses(
        (status = 200, description = "服务账号已删除", body = ryframe_http::ApiEmptyResponse),
        (status = 400, description = "账号 ID 无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "没有服务账号删除权限"),
        (status = 404, description = "服务账号不存在"),
        (status = 503, description = "服务账号功能未启用或数据库不可用")
    ), security(("bearer" = [])))]
async fn remove_account(
    State(state): State<AppState>,
    actor: RequestPrincipal,
    Path(id): Path<String>,
) -> HttpResult<Json<ApiResponse<()>>> {
    service(&state)?
        .delete_account(&actor, parse_positive_id(&id, "服务账号")?)
        .await?;
    Ok(Json(ApiResponse::success_no_data()))
}

#[get("/{id}/roles")]
#[capability("system.service_accounts")]
#[perm("system:service-account:role")]
#[utoipa::path(get, path = "/api/v1/system/service-accounts/{id}/roles", tag = "服务账号",
    params(("id" = String, Path)),
    responses(
        (status = 200, description = "服务账号角色 ID", body = ApiResponse<Vec<String>>),
        (status = 400, description = "账号 ID 无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "没有服务账号角色权限"),
        (status = 404, description = "服务账号不存在"),
        (status = 503, description = "服务账号功能未启用或数据库不可用")
    ), security(("bearer" = [])))]
async fn account_roles(
    State(state): State<AppState>,
    actor: RequestPrincipal,
    Path(id): Path<String>,
) -> HttpResult<Json<ApiResponse<Vec<String>>>> {
    let roles = service(&state)?
        .account_role_ids(&actor, parse_positive_id(&id, "服务账号")?)
        .await?;
    Ok(Json(ApiResponse::success(roles)))
}

#[put("/{id}/roles")]
#[capability("system.service_accounts")]
#[perm("system:service-account:role")]
#[utoipa::path(put, path = "/api/v1/system/service-accounts/{id}/roles", tag = "服务账号",
    params(("id" = String, Path)), request_body = ReplaceServiceAccountRolesDto,
    responses(
        (status = 200, description = "服务账号角色已替换", body = ryframe_http::ApiEmptyResponse),
        (status = 400, description = "账号或角色 ID 无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "没有服务账号角色权限，或选择了超级角色"),
        (status = 404, description = "服务账号或角色不存在"),
        (status = 503, description = "服务账号功能未启用或数据库不可用")
    ), security(("bearer" = [])))]
async fn replace_account_roles(
    State(state): State<AppState>,
    actor: RequestPrincipal,
    Path(id): Path<String>,
    Json(request): Json<ReplaceServiceAccountRolesDto>,
) -> HttpResult<Json<ApiResponse<()>>> {
    request.validate()?;
    service(&state)?
        .replace_account_roles(
            &actor,
            parse_positive_id(&id, "服务账号")?,
            parse_i64_strings(&request.role_ids)?,
        )
        .await?;
    Ok(Json(ApiResponse::success_no_data()))
}

#[get("/{id}/credentials")]
#[capability("system.service_accounts")]
#[perm("system:service-account:list")]
#[utoipa::path(get, path = "/api/v1/system/service-accounts/{id}/credentials", tag = "服务账号",
    params(("id" = String, Path)),
    responses(
        (status = 200, description = "API Key 元数据列表", body = ApiResponse<Vec<ServiceCredentialVo>>),
        (status = 400, description = "账号 ID 无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "没有服务账号查看权限"),
        (status = 404, description = "服务账号不存在"),
        (status = 503, description = "服务账号功能未启用或数据库不可用")
    ), security(("bearer" = [])))]
async fn list_credentials(
    State(state): State<AppState>,
    actor: RequestPrincipal,
    Path(id): Path<String>,
) -> HttpResult<Json<ApiResponse<Vec<ServiceCredentialVo>>>> {
    let values = service(&state)?
        .list_credentials(&actor, parse_positive_id(&id, "服务账号")?)
        .await?;
    Ok(Json(ApiResponse::success(
        values.into_iter().map(Into::into).collect(),
    )))
}

#[post("/{id}/credentials")]
#[capability("system.service_accounts")]
#[perm("system:service-account:key-rotate")]
#[utoipa::path(post, path = "/api/v1/system/service-accounts/{id}/credentials", tag = "服务账号",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = CreateServiceCredentialDto,
    responses(
        (status = 200, description = "API Key 已创建；Secret 只显示一次", body = ApiResponse<CreatedServiceCredentialVo>),
        (status = 400, description = "请求参数或幂等键无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "没有 API Key 轮换权限"),
        (status = 404, description = "服务账号不存在"),
        (status = 409, description = "有效 Key 已达上限或幂等冲突"),
        (status = 503, description = "服务账号功能、Pepper 或数据库不可用")
    ), security(("bearer" = [])))]
async fn create_credential(
    State(state): State<AppState>,
    actor: RequestPrincipal,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CreateServiceCredentialDto>,
) -> HttpResult<Response> {
    request.validate()?;
    let value: CreatedServiceCredentialVo = service(&state)?
        .create_credential(
            &actor,
            parse_positive_id(&id, "服务账号")?,
            CreateCredentialCommand {
                label: request.label,
                expires_at: request.expires_at,
                idempotency_key: idempotency_key_value(&headers)?,
            },
        )
        .await?
        .into();
    Ok(one_time_response(value))
}

#[delete("/{id}/credentials/{credential_id}")]
#[capability("system.service_accounts")]
#[perm("system:service-account:key-revoke")]
#[utoipa::path(delete, path = "/api/v1/system/service-accounts/{id}/credentials/{credential_id}", tag = "服务账号",
    params(("id" = String, Path), ("credential_id" = String, Path)),
    responses(
        (status = 200, description = "API Key 已撤销", body = ryframe_http::ApiEmptyResponse),
        (status = 400, description = "ID 无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "没有 API Key 撤销权限"),
        (status = 404, description = "服务账号或 API Key 不存在"),
        (status = 409, description = "API Key 已撤销"),
        (status = 503, description = "服务账号功能未启用或数据库不可用")
    ), security(("bearer" = [])))]
async fn revoke_credential(
    State(state): State<AppState>,
    actor: RequestPrincipal,
    Path((id, credential_id)): Path<(String, String)>,
) -> HttpResult<Json<ApiResponse<()>>> {
    service(&state)?
        .revoke_credential(
            &actor,
            parse_positive_id(&id, "服务账号")?,
            parse_positive_id(&credential_id, "API Key")?,
        )
        .await?;
    Ok(Json(ApiResponse::success_no_data()))
}

#[get("/")]
#[capability("system.service_accounts")]
#[perm("system:service-delegation:list")]
#[utoipa::path(get, path = "/api/v1/system/service-delegations", tag = "服务委托",
    params(ServiceResourcePageQuery),
    responses(
        (status = 200, description = "当前租户委托列表", body = ApiPageResponse<ServiceDelegationVo>),
        (status = 400, description = "分页参数无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "没有委托查看权限"),
        (status = 503, description = "服务账号功能未启用或数据库不可用")
    ), security(("bearer" = [])))]
async fn list_delegations(
    State(state): State<AppState>,
    actor: RequestPrincipal,
    Query(query): Query<ServiceResourcePageQuery>,
) -> HttpResult<Json<ApiPageResponse<ServiceDelegationVo>>> {
    let page = service(&state)?
        .list_delegations(&actor, query.into_page(&state.config.pagination)?)
        .await?;
    Ok(Json(ApiPageResponse::page(
        page.records.into_iter().map(Into::into).collect(),
        page.total,
        page.page,
        page.page_size,
        state.config.pagination.max_page_size,
    )))
}

#[delete("/{id}")]
#[capability("system.service_accounts")]
#[perm("system:service-delegation:revoke")]
#[utoipa::path(delete, path = "/api/v1/system/service-delegations/{id}", tag = "服务委托",
    params(("id" = String, Path)),
    responses(
        (status = 200, description = "委托已撤销", body = ryframe_http::ApiEmptyResponse),
        (status = 400, description = "委托 ID 无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "没有委托撤销权限"),
        (status = 404, description = "委托不存在"),
        (status = 409, description = "委托已撤销"),
        (status = 503, description = "服务账号功能未启用或数据库不可用")
    ), security(("bearer" = [])))]
async fn revoke_delegation(
    State(state): State<AppState>,
    actor: RequestPrincipal,
    Path(id): Path<String>,
) -> HttpResult<Json<ApiResponse<()>>> {
    service(&state)?
        .revoke_managed_delegation(&actor, parse_positive_id(&id, "委托")?)
        .await?;
    Ok(Json(ApiResponse::success_no_data()))
}

#[get("/")]
#[capability("system.service_accounts")]
#[perm("system:service-access-audit:list")]
#[utoipa::path(get, path = "/api/v1/system/service-access-audits", tag = "服务访问审计",
    params(ServiceResourcePageQuery),
    responses(
        (status = 200, description = "Agent 访问审计列表", body = ApiPageResponse<ServiceAccessAuditVo>),
        (status = 400, description = "分页参数无效"),
        (status = 401, description = "未认证"),
        (status = 403, description = "没有服务访问审计权限"),
        (status = 503, description = "服务账号功能未启用或数据库不可用")
    ), security(("bearer" = [])))]
async fn list_access_audits(
    State(state): State<AppState>,
    actor: RequestPrincipal,
    Query(query): Query<ServiceResourcePageQuery>,
) -> HttpResult<Json<ApiPageResponse<ServiceAccessAuditVo>>> {
    let page = service(&state)?
        .list_access_audits(&actor, query.into_page(&state.config.pagination)?)
        .await?;
    Ok(Json(ApiPageResponse::page(
        page.records.into_iter().map(Into::into).collect(),
        page.total,
        page.page,
        page.page_size,
        state.config.pagination.max_page_size,
    )))
}

fn service(state: &AppState) -> HttpResult<&ServiceAccountService> {
    state
        .services
        .service_accounts
        .as_deref()
        .ok_or_else(|| AppError::ServiceUnavailable("服务账号功能未启用".into()).into())
}

fn parse_positive_id(value: &str, label: &str) -> HttpResult<i64> {
    value
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| AppError::Validation(format!("{label} ID 无效")).into())
}

fn parse_optional_positive_id(value: Option<&str>, label: &str) -> HttpResult<Option<i64>> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| parse_positive_id(value, label))
        .transpose()
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
