use std::net::{IpAddr, Ipv4Addr};

use crate::http::{ApiPageResponse, ApiResponse, HttpAppError};
use axum::{
    Router,
    body::Body,
    extract::{Extension, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use ryframe_application::agent::{AgentCapability, AgentRequest};
use ryframe_kernel::AppError;
use ryframe_macro::{get, route};
use ryframe_utils::ip::ClientIp;

use crate::{
    dto::agent_dto::{
        AgentCapabilityResponse, AgentDepartmentResponse, AgentDictionaryResponse, AgentPageQuery,
        AgentPostResponse, AgentUserResponse,
    },
    middleware::{request_id::RequestId, response_envelope::PrebuiltApiEnvelope},
    request_locale::RequestLocale,
    state::AppState,
};

pub fn agent_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(capabilities))
        .merge(route!(users))
        .merge(route!(departments))
        .merge(route!(posts))
        .merge(route!(dictionary))
        .fallback(unregistered)
        .method_not_allowed_fallback(unregistered)
        .with_state(state)
}

#[get("/capabilities")]
#[capability("system.service_accounts")]
#[utoipa::path(get, path = "/api/v1/agent/v1/capabilities", tag = "Agent API",
    responses(
        (status = 200, description = "当前身份真正可用的编译期能力", body = ApiResponse<Vec<AgentCapabilityResponse>>),
        (status = 400, description = "请求参数无效"),
        (status = 401, description = "Agent 凭据无效"),
        (status = 403, description = "能力不可用"),
        (status = 413, description = "响应超过固定上限"),
        (status = 429, description = "请求频率或并发超过上限"),
        (status = 503, description = "授权、限流、数据库或审计服务不可用")
    ), security(("ryframeApiKey" = []), ("ryframeApiKey" = [], "delegationToken" = [])))]
async fn capabilities(State(state): State<AppState>, context: AgentHttpContext) -> Response {
    execute(&state, AgentCapability::Capabilities, context, None).await
}

#[get("/directory/users")]
#[capability("system.service_accounts")]
#[utoipa::path(get, path = "/api/v1/agent/v1/directory/users", tag = "Agent API",
    params(AgentPageQuery),
    responses(
        (status = 200, description = "双主体数据范围交集内的用户", body = ApiPageResponse<AgentUserResponse>),
        (status = 400, description = "分页或过滤参数无效"),
        (status = 401, description = "Agent 凭据无效"),
        (status = 403, description = "权限交集不足"),
        (status = 413, description = "响应超过固定上限"),
        (status = 429, description = "请求频率或并发超过上限"),
        (status = 503, description = "授权、限流、数据库或审计服务不可用")
    ), security(("ryframeApiKey" = []), ("ryframeApiKey" = [], "delegationToken" = [])))]
async fn users(State(state): State<AppState>, context: AgentHttpContext) -> Response {
    execute(&state, AgentCapability::DirectoryUsers, context, None).await
}

#[get("/directory/departments")]
#[capability("system.service_accounts")]
#[utoipa::path(get, path = "/api/v1/agent/v1/directory/departments", tag = "Agent API",
    params(AgentPageQuery),
    responses(
        (status = 200, description = "双主体数据范围交集内的部门", body = ApiPageResponse<AgentDepartmentResponse>),
        (status = 400, description = "分页或过滤参数无效"),
        (status = 401, description = "Agent 凭据无效"),
        (status = 403, description = "权限交集不足"),
        (status = 413, description = "响应超过固定上限"),
        (status = 429, description = "请求频率或并发超过上限"),
        (status = 503, description = "授权、限流、数据库或审计服务不可用")
    ), security(("ryframeApiKey" = []), ("ryframeApiKey" = [], "delegationToken" = [])))]
async fn departments(State(state): State<AppState>, context: AgentHttpContext) -> Response {
    execute(&state, AgentCapability::DirectoryDepartments, context, None).await
}

#[get("/directory/posts")]
#[capability("system.service_accounts")]
#[utoipa::path(get, path = "/api/v1/agent/v1/directory/posts", tag = "Agent API",
    params(AgentPageQuery),
    responses(
        (status = 200, description = "双方数据范围均为全部时可见的岗位", body = ApiPageResponse<AgentPostResponse>),
        (status = 400, description = "分页或过滤参数无效"),
        (status = 401, description = "Agent 凭据无效"),
        (status = 403, description = "权限交集不足"),
        (status = 413, description = "响应超过固定上限"),
        (status = 429, description = "请求频率或并发超过上限"),
        (status = 503, description = "授权、限流、数据库或审计服务不可用")
    ), security(("ryframeApiKey" = []), ("ryframeApiKey" = [], "delegationToken" = [])))]
async fn posts(State(state): State<AppState>, context: AgentHttpContext) -> Response {
    execute(&state, AgentCapability::DirectoryPosts, context, None).await
}

#[get("/reference/dictionaries/{type_code}")]
#[capability("system.service_accounts")]
#[utoipa::path(get, path = "/api/v1/agent/v1/reference/dictionaries/{type_code}", tag = "Agent API",
    params(("type_code" = String, Path), AgentPageQuery),
    responses(
        (status = 200, description = "双方数据范围均为全部时可见的启用字典", body = ApiResponse<AgentDictionaryResponse>),
        (status = 400, description = "字典类型或分页参数无效"),
        (status = 401, description = "Agent 凭据无效"),
        (status = 403, description = "权限交集不足"),
        (status = 404, description = "字典类型不存在"),
        (status = 413, description = "响应超过固定上限"),
        (status = 429, description = "请求频率或并发超过上限"),
        (status = 503, description = "授权、限流、数据库或审计服务不可用")
    ), security(("ryframeApiKey" = []), ("ryframeApiKey" = [], "delegationToken" = [])))]
async fn dictionary(
    State(state): State<AppState>,
    Path(type_code): Path<String>,
    context: AgentHttpContext,
) -> Response {
    execute(
        &state,
        AgentCapability::ReferenceDictionary,
        context,
        Some(type_code),
    )
    .await
}

struct AgentHttpContext {
    authorization: Option<String>,
    delegation: Option<String>,
    page: u64,
    page_size: u64,
    validation_error: Option<String>,
    request_id: String,
    client_ip: IpAddr,
    user_agent: Option<String>,
    locale: ryframe_kernel::Locale,
    started_at: chrono::DateTime<Utc>,
}

impl<S> axum::extract::FromRequestParts<S> for AgentHttpContext
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        let started_at = Utc::now();
        let parsed = axum::extract::Query::<AgentPageQuery>::try_from_uri(&parts.uri);
        let (page, page_size, validation_error) = match parsed {
            Ok(query) => (query.page.unwrap_or(1), query.page_size.unwrap_or(20), None),
            Err(_) => (1, 20, Some("Agent 查询参数无效".to_owned())),
        };
        let headers = &parts.headers;
        let has_request_body = headers.contains_key(header::TRANSFER_ENCODING)
            || headers
                .get(header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value != "0");
        let validation_error = validation_error
            .or_else(|| has_request_body.then(|| "Agent 只读 GET 请求不得携带请求体".to_owned()));
        Ok(Self {
            authorization: header_value(headers, header::AUTHORIZATION),
            delegation: header_value_name(headers, "X-RyFrame-Delegation"),
            page,
            page_size,
            validation_error,
            request_id: parts
                .extensions
                .get::<RequestId>()
                .map(|value| value.0.clone())
                .unwrap_or_else(|| uuid::Uuid::now_v7().to_string()),
            client_ip: parts
                .extensions
                .get::<ClientIp>()
                .map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |value| value.0),
            user_agent: header_value(headers, header::USER_AGENT).map(|value| truncate(value, 512)),
            locale: parts
                .extensions
                .get::<RequestLocale>()
                .map_or(ryframe_kernel::Locale::ZhCn, |value| value.0),
            started_at,
        })
    }
}

async fn execute(
    state: &AppState,
    capability: AgentCapability,
    context: AgentHttpContext,
    type_code: Option<String>,
) -> Response {
    let Some(service) = state.services.agent.as_deref() else {
        return HttpAppError::from(AppError::ServiceUnavailable("Agent API 未启用".into()))
            .into_response();
    };
    let success_message = state
        .localizer
        .translate(context.locale, crate::http::QUERY_SUCCESS_MESSAGE_KEY);
    match service
        .execute(AgentRequest {
            capability,
            authorization: context.authorization,
            delegation: context.delegation,
            page: context.page,
            page_size: context.page_size,
            type_code,
            request_id: context.request_id,
            client_ip: context.client_ip,
            user_agent: context.user_agent,
            success_message,
            started_at: context.started_at,
            validation_error: context.validation_error,
        })
        .await
    {
        Ok(success) => {
            let mut response = Response::new(Body::from(success.body));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
                .headers_mut()
                .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
            response.extensions_mut().insert(PrebuiltApiEnvelope);
            response
        }
        Err(error) => HttpAppError::from(error).into_response(),
    }
}

async fn unregistered(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    client_ip: Option<Extension<ClientIp>>,
    headers: HeaderMap,
) -> Response {
    if let Some(service) = state.services.agent.as_deref() {
        match service
            .audit_unregistered(
                request_id.0,
                client_ip.map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |value| value.0.0),
                header_value(&headers, header::USER_AGENT).map(|value| truncate(value, 512)),
                Utc::now(),
            )
            .await
        {
            Ok(()) => {}
            Err(error @ (AppError::RateLimited(_, _) | AppError::ServiceUnavailable(_))) => {
                return HttpAppError::from(error).into_response();
            }
            Err(_) => {
                return HttpAppError::from(AppError::ServiceUnavailable(
                    "Agent 访问审计不可用".into(),
                ))
                .into_response();
            }
        }
    }
    StatusCode::NOT_FOUND.into_response()
}

fn header_value(headers: &HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn header_value_name(headers: &HeaderMap, name: &'static str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn truncate(value: String, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}
