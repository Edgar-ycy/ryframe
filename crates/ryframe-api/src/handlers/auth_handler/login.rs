use std::net::SocketAddr;

use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use ryframe_core::TenantContext;
use ryframe_http::{ApiResponse, HttpResult};
use ryframe_kernel::{AppError, AppResult};
use ryframe_service::system::{LoginStatus, RecordLoginCommand};
use validator::Validate;

use super::{
    context::{build_session_context, login_actor},
    cookies::{refresh_cookie, refresh_cookie_session_id},
    guards::{
        enforce_login_rate_limit, extract_ip, extract_user_agent, tenant_id, validate_auth_origin,
        verify_captcha_if_enabled, verify_csrf,
    },
};
use crate::{
    dto::auth_dto::{LoginRequest, LoginResponse},
    state::AppState,
};

pub(super) async fn record_login_success(
    state: &AppState,
    tenant_id: &str,
    username: &str,
    ip: &str,
    user_agent: &str,
) {
    if let Err(error) = record_login_event(
        state,
        tenant_id,
        username,
        ip,
        user_agent,
        LoginStatus::Success,
        None,
    )
    .await
    {
        tracing::error!(%error, "记录登录成功日志失败");
    }
}

pub(super) async fn record_login_failure_log(
    state: &AppState,
    tenant_id: &str,
    username: &str,
    ip: &str,
    user_agent: &str,
    error: &AppError,
) {
    if let Err(record_error) = record_login_event(
        state,
        tenant_id,
        username,
        ip,
        user_agent,
        LoginStatus::Failure,
        Some(error.to_string()),
    )
    .await
    {
        tracing::error!(%record_error, "记录登录失败日志失败");
    }
}

async fn record_login_event(
    state: &AppState,
    tenant_id: &str,
    username: &str,
    ip: &str,
    user_agent: &str,
    status: LoginStatus,
    message: Option<String>,
) -> AppResult<()> {
    state
        .services
        .login_info
        .record_login(RecordLoginCommand {
            tenant_id: tenant_id.into(),
            user_name: username.into(),
            ipaddr: ip.into(),
            browser: ryframe_utils::user_agent::parse_browser(user_agent),
            os: ryframe_utils::user_agent::parse_os(user_agent),
            status,
            message,
        })
        .await
}

async fn add_online_user(
    state: &AppState,
    tenant_id: &str,
    result: &ryframe_service::LoginResult,
    ip: &str,
    user_agent: &str,
) -> AppResult<()> {
    use ryframe_service::system::UserSession;

    let login_location = ryframe_utils::ip::get_ip_location(ip);
    let now = chrono::Utc::now();

    state
        .services
        .online_user
        .add_user(UserSession {
            sid: result.sid.clone(),
            tenant_id: tenant_id.to_owned(),
            user_id: result.user_id,
            username: result.user_info.username.clone(),
            dept_name: result.user_info.dept_name.clone(),
            ipaddr: ip.to_string(),
            login_location,
            browser: ryframe_utils::user_agent::parse_browser(user_agent),
            os: ryframe_utils::user_agent::parse_os(user_agent),
            login_time: now,
            last_access_time: now,
            absolute_exp: result.refresh_expires_at as i64,
        })
        .await
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "认证",
    request_body = LoginRequest,
    params(("X-CSRF-Token" = String, Header, description = "已签名的 CSRF 挑战令牌")),
    responses(
        (status = 200, description = "登录成功", body = ApiResponse<LoginResponse>),
        (status = 400, description = "参数校验失败"),
        (status = 401, description = "用户名或密码错误"),
        (status = 409, description = "登录设备数量已达到安全上限"),
        (status = 503, description = "会话元数据或 Redis 服务不可用")
    )
)]
pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    tenant_context: Option<Extension<TenantContext>>,
    Json(req): Json<LoginRequest>,
) -> HttpResult<Response> {
    req.validate()?;
    validate_auth_origin(&state, &headers)?;
    let csrf_sid = refresh_cookie_session_id(&jar, &state.config.auth.jwt_secret);
    verify_csrf(
        &jar,
        &headers,
        &state.config.auth.jwt_secret,
        csrf_sid.as_deref(),
    )?;

    let ip = extract_ip(&state, &headers, addr);
    let user_agent = extract_user_agent(&headers);
    let tenant_id = tenant_id(&state, tenant_context, &headers)?;

    if let Some(response) = enforce_login_rate_limit(&state, &tenant_id, &req.username, &ip).await?
    {
        return Ok(response);
    }

    verify_captcha_if_enabled(&state, &tenant_id, &req).await?;

    state
        .services
        .auth
        .check_brute_force(&tenant_id, &req.username, &ip)
        .await?;

    match state
        .services
        .auth
        .login(&tenant_id, &req.username, &req.password)
        .await
    {
        Ok(result) => {
            if let Err(error) = state
                .services
                .auth
                .clear_login_failures(&tenant_id, &req.username, &ip)
                .await
            {
                ryframe_middleware::metrics::record_redis_degraded("login_protection");
                if let Err(revoke_error) = state
                    .services
                    .auth
                    .refresh_sessions()
                    .revoke_for_user(&tenant_id, result.user_id, &result.sid)
                    .await
                {
                    tracing::error!(%revoke_error, sid = %result.sid, "登录保护状态失败后的会话补偿撤销失败");
                }
                return Err(error.into());
            }
            let actor = login_actor(result.user_id, &result.user_info);
            let session_context = match build_session_context(&state, &actor).await {
                Ok(context) => context,
                Err(error) => {
                    if let Err(revoke_error) = state
                        .services
                        .auth
                        .refresh_sessions()
                        .revoke_for_user(&tenant_id, result.user_id, &result.sid)
                        .await
                    {
                        tracing::error!(%revoke_error, sid = %result.sid, "会话上下文构建失败后的会话补偿撤销失败");
                    }
                    return Err(error.into());
                }
            };
            if let Err(error) = add_online_user(&state, &tenant_id, &result, &ip, user_agent).await
            {
                ryframe_middleware::metrics::record_redis_degraded("login_session_metadata");
                if let Err(revoke_error) = state
                    .services
                    .auth
                    .refresh_sessions()
                    .revoke_for_user(&tenant_id, result.user_id, &result.sid)
                    .await
                {
                    tracing::error!(%revoke_error, sid = %result.sid, "登录元数据失败后的会话补偿撤销失败");
                }
                return Err(error.into());
            }
            record_login_success(&state, &tenant_id, &req.username, &ip, user_agent).await;

            Ok((
                jar.add(refresh_cookie(
                    &result.refresh_token,
                    result.refresh_expires_at,
                    state.config.environment,
                )),
                Json(ApiResponse::success(LoginResponse::new(
                    result,
                    session_context,
                ))),
            )
                .into_response())
        }
        Err(error) => {
            if matches!(&error, AppError::ServiceUnavailable(_)) {
                ryframe_middleware::metrics::record_redis_degraded("login_session");
            }
            if matches!(&error, AppError::Authentication(_))
                && let Err(record_error) = state
                    .services
                    .auth
                    .record_login_failure(&tenant_id, &req.username, &ip)
                    .await
            {
                ryframe_middleware::metrics::record_redis_degraded("login_protection");
                return Err(record_error.into());
            }
            record_login_failure_log(&state, &tenant_id, &req.username, &ip, user_agent, &error)
                .await;
            Err(error.into())
        }
    }
}
