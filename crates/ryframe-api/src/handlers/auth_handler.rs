use std::net::SocketAddr;

use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::HeaderMap,
};
use ryframe_auth::{RequestPrincipal, jwt::Claims};
use ryframe_common::{ApiResponse, AppError, AppResult};
use ryframe_core::TenantContext;
use ryframe_service::{
    UserInfo,
    system::{LoginStatus, RecordLoginCommand},
};
use validator::Validate;

use crate::dto::auth_dto::{
    CompletePasswordResetRequest, LoginRequest, LoginResponse, RefreshRequest,
};
use crate::{handler_utils::tenant_id_from_headers, state::AppState};

// ==================== 登录辅助：参数提取 ====================

/// 提取客户端 IP
fn extract_ip(headers: &HeaderMap, remote_addr: &str) -> String {
    ryframe_common::utils::ip::get_client_ip(headers, remote_addr)
}

/// 提取 User-Agent
fn extract_user_agent(headers: &HeaderMap) -> &str {
    headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

async fn verify_captcha_if_enabled(
    state: &AppState,
    tenant_id: &str,
    req: &LoginRequest,
) -> AppResult<()> {
    let captcha_enabled = state
        .services
        .config
        .find_public_value(tenant_id, "sys.account.captchaEnabled")
        .await
        .ok()
        .flatten()
        .map(|value| value == "true")
        .unwrap_or(true);
    if !captcha_enabled {
        return Ok(());
    }

    let captcha_id = req
        .captcha_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Validation("验证码ID不能为空".into()))?;
    let captcha_code = req
        .captcha_code
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Validation("验证码不能为空".into()))?;
    let valid = state
        .services
        .captcha
        .verify(captcha_id, captcha_code)
        .await;
    if !valid {
        return Err(AppError::Validation("验证码错误或已过期".into()));
    }
    Ok(())
}

// ==================== 认证接口 ====================

/// 记录登录成功日志
async fn record_login_success(
    state: &AppState,
    tenant_id: &str,
    username: &str,
    ip: &str,
    ua: &str,
) {
    if let Err(e) = state
        .services
        .login_info
        .record_login(RecordLoginCommand {
            tenant_id: tenant_id.into(),
            user_name: username.into(),
            ipaddr: ip.into(),
            browser: ryframe_common::utils::user_agent::parse_browser(ua),
            os: ryframe_common::utils::user_agent::parse_os(ua),
            status: LoginStatus::Success,
            message: None,
        })
        .await
    {
        tracing::error!("记录登录成功日志失败: {}", e);
    }
}

/// 记录登录失败日志
async fn record_login_failure_log(
    state: &AppState,
    tenant_id: &str,
    username: &str,
    ip: &str,
    ua: &str,
    err: &AppError,
) {
    if let Err(e) = state
        .services
        .login_info
        .record_login(RecordLoginCommand {
            tenant_id: tenant_id.into(),
            user_name: username.into(),
            ipaddr: ip.into(),
            browser: ryframe_common::utils::user_agent::parse_browser(ua),
            os: ryframe_common::utils::user_agent::parse_os(ua),
            status: LoginStatus::Failure,
            message: Some(err.to_string()),
        })
        .await
    {
        tracing::error!("记录登录失败日志失败: {}", e);
    }
}

/// 添加在线用户
async fn add_online_user(
    state: &AppState,
    tenant_id: &str,
    result: &ryframe_service::LoginResult,
    ip: &str,
    ua: &str,
) {
    use ryframe_service::system::UserSession;

    let user_id: i64 = result.user_info.id.parse().unwrap_or(0);
    let login_location = ryframe_common::utils::ip::get_ip_location(ip);
    let now = chrono::Utc::now();

    state
        .services
        .online_user
        .add_user(UserSession {
            token_id: result.token_id.clone(),
            tenant_id: tenant_id.to_owned(),
            user_id,
            username: result.user_info.username.clone(),
            dept_name: result.user_info.dept_name.clone(),
            ipaddr: ip.to_string(),
            login_location,
            browser: ryframe_common::utils::user_agent::parse_browser(ua),
            os: ryframe_common::utils::user_agent::parse_os(ua),
            login_time: now,
            last_access_time: now,
        })
        .await;
}

/// 检查用户是否被强制下线
async fn check_force_logout(state: &AppState, refresh_token: &str) -> AppResult<()> {
    if let Ok(claims) =
        ryframe_auth::jwt::decode_token(refresh_token, &state.config.auth.jwt_secret)
    {
        let force_logout_key = format!("force_logout:{}:user:{}", claims.tenant_id, claims.sub);
        if state
            .token_blacklist
            .is_blacklisted(&force_logout_key)
            .await
        {
            return Err(AppError::Authentication(
                "账号已被强制下线，请重新登录".into(),
            ));
        }
    }
    Ok(())
}

/// 用户登录
///
/// POST /api/v1/auth/login
#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "认证",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "登录成功", body = ApiResponse<LoginResponse>),
        (status = 400, description = "参数校验失败"),
        (status = 401, description = "用户名或密码错误")
    )
)]
pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    tenant_context: Option<Extension<TenantContext>>,
    Json(req): Json<LoginRequest>,
) -> AppResult<Json<ApiResponse<LoginResponse>>> {
    req.validate()?;

    // HTTP 安全检查：验证码

    // HTTP 参数提取
    let ip = extract_ip(&headers, &addr.to_string());
    let ua = extract_user_agent(&headers);
    let tenant_id = tenant_context
        .map(|Extension(context)| context.tenant_id)
        .map(Ok)
        .unwrap_or_else(|| tenant_id_from_headers(&headers))?;

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
            state
                .services
                .auth
                .clear_login_failures(&tenant_id, &req.username, &ip)
                .await;
            let force_logout_key =
                format!("force_logout:{}:user:{}", tenant_id, result.user_info.id);
            state.token_blacklist.remove(&force_logout_key).await;
            // 记录登录成功日志
            record_login_success(&state, &tenant_id, &req.username, &ip, ua).await;
            // 添加在线用户
            add_online_user(&state, &tenant_id, &result, &ip, ua).await;

            Ok(Json(ApiResponse::success(LoginResponse::from(result))))
        }
        Err(e) => {
            // 登录失败：记录失败次数 + 记录失败日志
            state
                .services
                .auth
                .record_login_failure(&tenant_id, &req.username, &ip)
                .await;
            record_login_failure_log(&state, &tenant_id, &req.username, &ip, ua, &e).await;
            Err(e)
        }
    }
}

/// 用户登出
///
/// POST /api/v1/auth/logout
#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "认证",
    responses(
        (status = 200, description = "登出成功", body = ryframe_common::ApiEmptyResponse),
    ),
    security(("bearer" = []))
)]
pub async fn logout(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> AppResult<Json<ApiResponse<()>>> {
    let now = chrono::Utc::now().timestamp() as usize;
    let remaining = if claims.exp > now {
        (claims.exp - now) as u64
    } else {
        0
    };
    if remaining > 0 {
        state
            .token_blacklist
            .blacklist(&claims.jti, remaining)
            .await;
    }

    // 从在线用户中移除
    state
        .services
        .online_user
        .remove_user(&claims.tenant_id, &claims.jti)
        .await;
    Ok(Json(ApiResponse::<()>::success_no_data()))
}

/// 刷新令牌
///
/// POST /api/v1/auth/refresh
#[utoipa::path(
    post,
    path = "/api/v1/auth/refresh",
    tag = "认证",
    request_body = RefreshRequest,
    responses(
        (status = 200, description = "刷新成功", body = ApiResponse<LoginResponse>),
        (status = 401, description = "令牌无效或已过期")
    )
)]
pub async fn refresh(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<RefreshRequest>,
) -> AppResult<Json<ApiResponse<LoginResponse>>> {
    // 检查强退
    check_force_logout(&state, &req.refresh_token).await?;

    // HTTP 参数提取
    let ip = extract_ip(&headers, &addr.to_string());
    let ua = extract_user_agent(&headers);

    match state.services.auth.refresh_token(&req.refresh_token).await {
        Ok(result) => {
            record_login_success(
                &state,
                &result.user_info.tenant_id,
                &result.user_info.username,
                &ip,
                ua,
            )
            .await;
            Ok(Json(ApiResponse::success(LoginResponse::from(result))))
        }
        Err(e) => {
            if let Ok(claims) =
                ryframe_auth::jwt::decode_token(&req.refresh_token, &state.config.auth.jwt_secret)
            {
                record_login_failure_log(&state, &claims.tenant_id, "unknown", &ip, ua, &e).await;
            }
            Err(e)
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/password-reset/complete",
    tag = "认证",
    request_body = CompletePasswordResetRequest,
    responses(
        (status = 200, description = "密码已重置", body = ryframe_common::ApiEmptyResponse),
        (status = 400, description = "参数校验失败"),
        (status = 401, description = "重置令牌无效")
    )
)]
pub async fn complete_password_reset(
    State(state): State<AppState>,
    Json(req): Json<CompletePasswordResetRequest>,
) -> AppResult<Json<ApiResponse<()>>> {
    req.validate()?;
    let request_id = req
        .request_id
        .parse::<i64>()
        .map_err(|_| AppError::Validation("无效的重置请求ID".into()))?;
    state
        .services
        .user
        .complete_password_reset_request(&req.tenant_id, request_id, &req.token, &req.new_password)
        .await?;
    Ok(Json(ApiResponse::success_no_data_with_msg(
        "password reset completed",
    )))
}

/// 获取当前用户信息
///
/// GET /api/v1/auth/me
#[utoipa::path(
    get,
    path = "/api/v1/auth/me",
    tag = "认证",
    responses(
        (status = 200, description = "用户信息", body = ApiResponse<UserInfo>),
        (status = 401, description = "未认证")
    ),
    security(("bearer" = []))
)]
pub async fn me(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> AppResult<Json<ApiResponse<UserInfo>>> {
    let user_info = state.services.auth.get_current_user(&current_user).await?;

    Ok(Json(ApiResponse::success(user_info)))
}
