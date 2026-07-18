use std::net::SocketAddr;

use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use ryframe_auth::RequestPrincipal;
use ryframe_common::{ApiResponse, AppError, AppResult};
use ryframe_core::TenantContext;
use ryframe_service::{
    UserInfo,
    system::{LoginStatus, RecordLoginCommand},
};
use validator::Validate;

use crate::dto::auth_dto::{
    CompletePasswordResetRequest, CsrfResponse, LoginRequest, LoginResponse,
};
use crate::{handler_utils::tenant_id_from_headers, state::AppState};

// ==================== 登录辅助：参数提取 ====================

/// 提取客户端 IP
fn extract_ip(state: &AppState, headers: &HeaderMap, remote_addr: SocketAddr) -> String {
    state
        .trusted_proxies
        .client_ip(headers, remote_addr.ip())
        .to_string()
}

/// 提取 User-Agent
fn extract_user_agent(headers: &HeaderMap) -> &str {
    headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

const REFRESH_COOKIE: &str = "ryframe_refresh_token";
const CSRF_COOKIE: &str = "ryframe_csrf";
const CSRF_HEADER: &str = "x-csrf-token";
const CSRF_TTL_SECONDS: usize = 300;

fn secure_cookies() -> bool {
    let environment = std::env::var("APP_ENV").ok();
    secure_cookies_for_environment(environment.as_deref())
}

fn secure_cookies_for_environment(environment: Option<&str>) -> bool {
    environment.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "prod" | "production"
        )
    })
}

fn auth_cookie_for_environment(
    name: &'static str,
    value: String,
    max_age_seconds: i64,
    environment: Option<&str>,
) -> Cookie<'static> {
    let max_age = cookie::time::Duration::seconds(max_age_seconds);
    Cookie::build((name, value))
        .path("/api/v1/auth")
        .http_only(true)
        .secure(secure_cookies_for_environment(environment))
        .same_site(SameSite::Lax)
        .max_age(max_age)
        .expires(cookie::time::OffsetDateTime::now_utc().saturating_add(max_age))
        .build()
}

fn refresh_cookie(token: &str, absolute_exp: usize) -> Cookie<'static> {
    let environment = std::env::var("APP_ENV").ok();
    refresh_cookie_for_environment(token, absolute_exp, environment.as_deref())
}

fn refresh_cookie_for_environment(
    token: &str,
    absolute_exp: usize,
    environment: Option<&str>,
) -> Cookie<'static> {
    let now = chrono::Utc::now().timestamp().max(0) as usize;
    let max_age = absolute_exp.saturating_sub(now).min(7 * 24 * 60 * 60) as i64;
    let mut cookie =
        auth_cookie_for_environment(REFRESH_COOKIE, token.to_owned(), max_age, environment);
    if let Ok(timestamp) = i64::try_from(absolute_exp)
        && let Ok(expires) = cookie::time::OffsetDateTime::from_unix_timestamp(timestamp)
    {
        // The browser deadline is the family deadline committed at login,
        // rather than a second wall-clock calculation that could slide at a
        // second boundary during rotation.
        cookie.set_expires(expires);
    }
    cookie
}

fn csrf_cookie(token: &str) -> Cookie<'static> {
    let environment = std::env::var("APP_ENV").ok();
    csrf_cookie_for_environment(token, environment.as_deref())
}

fn csrf_cookie_for_environment(token: &str, environment: Option<&str>) -> Cookie<'static> {
    auth_cookie_for_environment(
        CSRF_COOKIE,
        token.to_owned(),
        CSRF_TTL_SECONDS as i64,
        environment,
    )
}

fn removal_cookie(name: &'static str) -> Cookie<'static> {
    Cookie::build((name, ""))
        .path("/api/v1/auth")
        .http_only(true)
        .secure(secure_cookies())
        .same_site(SameSite::Lax)
        .removal()
        .build()
}

fn clear_auth_cookies(jar: CookieJar) -> CookieJar {
    jar.add(removal_cookie(REFRESH_COOKIE))
        .add(removal_cookie(CSRF_COOKIE))
}

fn verify_csrf(
    jar: &CookieJar,
    headers: &HeaderMap,
    secret: &str,
    expected_sid: Option<&str>,
) -> AppResult<String> {
    let header_token = headers
        .get(CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            ryframe_middleware::metrics::record_csrf_rejection();
            AppError::Authorization("missing CSRF challenge".into())
        })?;
    let cookie_token = jar.get(CSRF_COOKIE).map(Cookie::value).ok_or_else(|| {
        ryframe_middleware::metrics::record_csrf_rejection();
        AppError::Authorization("missing CSRF challenge cookie".into())
    })?;
    if header_token != cookie_token {
        ryframe_middleware::metrics::record_csrf_rejection();
        return Err(AppError::Authorization("CSRF challenge mismatch".into()));
    }
    let claims = ryframe_auth::jwt::decode_csrf(header_token, secret).inspect_err(|_| {
        ryframe_middleware::metrics::record_csrf_rejection();
    })?;
    if claims.sid.as_deref() != expected_sid {
        ryframe_middleware::metrics::record_csrf_rejection();
        return Err(AppError::Authorization(
            "CSRF challenge is not bound to this session".into(),
        ));
    }
    Ok(claims.jti)
}

fn decode_refresh_cookie(jar: &CookieJar, secret: &str) -> AppResult<ryframe_auth::jwt::Claims> {
    let token = jar
        .get(REFRESH_COOKIE)
        .map(Cookie::value)
        .ok_or_else(|| AppError::Authentication("missing refresh cookie".into()))?;
    let claims = ryframe_auth::jwt::decode_token(token, secret)?;
    if claims.token_type != "refresh" || claims.sid.is_empty() {
        return Err(AppError::Authentication("invalid refresh cookie".into()));
    }
    Ok(claims)
}

fn validate_auth_origin(state: &AppState, headers: &HeaderMap) -> AppResult<()> {
    match headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        Some(origin)
            if state
                .config
                .cors
                .allow_origins
                .iter()
                .any(|allowed| allowed == origin) =>
        {
            Ok(())
        }
        Some(_) => {
            ryframe_middleware::metrics::record_csrf_rejection();
            Err(AppError::Authorization(
                "request origin is not allowed".into(),
            ))
        }
        None if secure_cookies() => {
            ryframe_middleware::metrics::record_csrf_rejection();
            Err(AppError::Authorization(
                "Origin header is required in production".into(),
            ))
        }
        None => Ok(()),
    }
}

async fn enforce_login_rate_limit(
    state: &AppState,
    tenant_id: &str,
    username: &str,
    client_ip: &str,
) -> AppResult<Option<Response>> {
    if !state.config.rate_limit.enabled {
        return Ok(None);
    }
    let limit = state
        .config
        .rate_limit
        .api_limits
        .get("POST /api/v1/auth/login")
        .copied()
        .unwrap_or(5);
    let window = state.config.rate_limit.api_window_secs.max(1);
    let normalized_username = username.trim().to_lowercase();
    let principal_digest =
        ryframe_common::utils::key::stable_scope_digest(&[tenant_id, &normalized_username]);
    for (scope, key) in [
        (
            "login_principal",
            format!("auth:login:principal:{principal_digest}"),
        ),
        ("login_ip", format!("auth:login:ip:{client_ip}")),
    ] {
        let decision = state
            .rate_limiter
            .acquire(&key, window, limit)
            .await
            .map_err(|error| {
                tracing::error!(%error, "login rate limiter unavailable");
                ryframe_middleware::metrics::record_redis_degraded("login_rate_limit");
                AppError::ServiceUnavailable("rate limit service unavailable".into())
            })?;
        if !decision.allowed {
            ryframe_middleware::metrics::record_rate_limit_rejection(scope);
            let mut response = (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                Json(ApiResponse::<()>::fail(
                    429,
                    "too many login attempts; try again later".into(),
                )),
            )
                .into_response();
            response.headers_mut().insert(
                header::RETRY_AFTER,
                HeaderValue::from_str(&decision.retry_after_secs.to_string())
                    .unwrap_or_else(|_| HeaderValue::from_static("60")),
            );
            return Ok(Some(response));
        }
    }
    Ok(None)
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
        .await?
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
        .await
        .inspect_err(|_| {
            ryframe_middleware::metrics::record_redis_degraded("captcha_store");
        })?;
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
            sid: result.sid.clone(),
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
            absolute_exp: result.refresh_expires_at as i64,
        })
        .await;
}

/// 获取短期 CSRF challenge
///
/// GET /api/v1/auth/csrf
#[utoipa::path(
    get,
    path = "/api/v1/auth/csrf",
    tag = "认证",
    responses((status = 200, description = "CSRF challenge", body = ApiResponse<CsrfResponse>))
)]
pub async fn csrf(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> AppResult<Response> {
    validate_auth_origin(&state, &headers)?;
    let sid = jar
        .get(REFRESH_COOKIE)
        .map(Cookie::value)
        .and_then(|token| {
            ryframe_auth::jwt::decode_token(token, &state.config.auth.jwt_secret).ok()
        })
        .filter(|claims| claims.token_type == "refresh" && !claims.sid.is_empty())
        .map(|claims| claims.sid);
    let token = ryframe_auth::jwt::encode_csrf(
        &state.config.auth.jwt_secret,
        sid.as_deref(),
        CSRF_TTL_SECONDS,
    )?;
    let mut response = (
        jar.add(csrf_cookie(&token)),
        Json(ApiResponse::success(CsrfResponse {
            csrf_token: token,
            expires_in: CSRF_TTL_SECONDS,
        })),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "认证",
    request_body = LoginRequest,
    params(("X-CSRF-Token" = String, Header, description = "Signed CSRF challenge")),
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
    jar: CookieJar,
    tenant_context: Option<Extension<TenantContext>>,
    Json(req): Json<LoginRequest>,
) -> AppResult<Response> {
    req.validate()?;
    validate_auth_origin(&state, &headers)?;
    let csrf_sid = jar
        .get(REFRESH_COOKIE)
        .and_then(|cookie| {
            ryframe_auth::jwt::decode_token(cookie.value(), &state.config.auth.jwt_secret).ok()
        })
        .filter(|claims| claims.token_type == "refresh" && !claims.sid.is_empty())
        .map(|claims| claims.sid);
    verify_csrf(
        &jar,
        &headers,
        &state.config.auth.jwt_secret,
        csrf_sid.as_deref(),
    )?;

    // HTTP 安全检查：验证码

    // HTTP 参数提取
    let ip = extract_ip(&state, &headers, addr);
    let ua = extract_user_agent(&headers);
    let tenant_id = tenant_context
        .map(|Extension(context)| context.tenant_id)
        .map(Ok)
        .unwrap_or_else(|| tenant_id_from_headers(&headers))?;

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
                return Err(error);
            }
            // 记录登录成功日志
            record_login_success(&state, &tenant_id, &req.username, &ip, ua).await;
            // 添加在线用户
            add_online_user(&state, &tenant_id, &result, &ip, ua).await;

            Ok((
                jar.add(refresh_cookie(
                    &result.refresh_token,
                    result.refresh_expires_at,
                )),
                Json(ApiResponse::success(LoginResponse::from(result))),
            )
                .into_response())
        }
        Err(e) => {
            if matches!(e, AppError::ServiceUnavailable(_)) {
                ryframe_middleware::metrics::record_redis_degraded("login_session");
            }
            // 登录失败：记录失败次数 + 记录失败日志
            if matches!(&e, AppError::Authentication(_))
                && let Err(error) = state
                    .services
                    .auth
                    .record_login_failure(&tenant_id, &req.username, &ip)
                    .await
            {
                ryframe_middleware::metrics::record_redis_degraded("login_protection");
                return Err(error);
            }
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
        (status = 403, description = "CSRF challenge 缺失、无效或与会话不匹配"),
        (status = 503, description = "Redis 会话或撤销服务不可用"),
    ),
    params(("X-CSRF-Token" = String, Header, description = "Signed challenge; bound to sid when a refresh cookie is present")),
    security((), ("refreshCookie" = []))
)]
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> AppResult<Response> {
    validate_auth_origin(&state, &headers)?;
    let has_refresh_cookie = jar.get(REFRESH_COOKIE).is_some();
    let decoded_refresh = decode_refresh_cookie(&jar, &state.config.auth.jwt_secret);
    verify_csrf(
        &jar,
        &headers,
        &state.config.auth.jwt_secret,
        decoded_refresh
            .as_ref()
            .ok()
            .map(|claims| claims.sid.as_str()),
    )?;
    let refresh_claims = match decoded_refresh {
        Ok(claims) => Some(claims),
        Err(AppError::Authentication(_)) if !has_refresh_cookie => None,
        Err(error) => {
            return Ok((clear_auth_cookies(jar), error).into_response());
        }
    };
    if let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        && let Ok(claims) = ryframe_auth::jwt::decode_token(value, &state.config.auth.jwt_secret)
        && claims.token_type == "access"
    {
        let now = chrono::Utc::now().timestamp().max(0) as usize;
        let remaining = claims.exp.saturating_sub(now) as u64;
        if remaining > 0 {
            state
                .token_blacklist
                .try_blacklist(&claims.jti, remaining)
                .await
                .inspect_err(|_| {
                    ryframe_middleware::metrics::record_redis_degraded("logout_revocation");
                })?;
        }
    }

    // 从在线用户中移除
    if let Some(claims) = refresh_claims {
        state
            .services
            .auth
            .refresh_sessions()
            .revoke(&claims.sid)
            .await
            .inspect_err(|_| {
                ryframe_middleware::metrics::record_redis_degraded("logout_session");
            })?;
        state
            .services
            .online_user
            .remove_user(&claims.tenant_id, &claims.sid)
            .await;
    }
    Ok((
        clear_auth_cookies(jar),
        Json(ApiResponse::<()>::success_no_data()),
    )
        .into_response())
}

/// 刷新令牌
///
/// POST /api/v1/auth/refresh
#[utoipa::path(
    post,
    path = "/api/v1/auth/refresh",
    params(("X-CSRF-Token" = String, Header, description = "Session-bound CSRF challenge")),
    security(("refreshCookie" = [])),
    tag = "认证",
    responses(
        (status = 200, description = "刷新成功", body = ApiResponse<LoginResponse>),
        (status = 401, description = "令牌无效、已过期、被撤销或确认重放"),
        (status = 403, description = "CSRF challenge 缺失、无效或与会话不匹配"),
        (status = 409, description = "另一个 rotation attempt 正在处理", headers(("Retry-After" = String, description = "再次刷新前等待的秒数"))),
        (status = 503, description = "Redis 会话服务不可用；显式重试必须复用原 X-CSRF-Token")
    )
)]
pub async fn refresh(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
) -> AppResult<Response> {
    validate_auth_origin(&state, &headers)?;
    // 检查强退
    let claims = match decode_refresh_cookie(&jar, &state.config.auth.jwt_secret) {
        Ok(claims) => claims,
        Err(error) => {
            return Ok((clear_auth_cookies(jar), error).into_response());
        }
    };
    let rotation_attempt_id = verify_csrf(
        &jar,
        &headers,
        &state.config.auth.jwt_secret,
        Some(&claims.sid),
    )?;
    let refresh_token = jar
        .get(REFRESH_COOKIE)
        .map(Cookie::value)
        .expect("refresh cookie was decoded above")
        .to_owned();
    // HTTP 参数提取
    let ip = extract_ip(&state, &headers, addr);
    let ua = extract_user_agent(&headers);

    match state
        .services
        .auth
        .refresh_token(&refresh_token, &rotation_attempt_id)
        .await
    {
        Ok(result) => {
            record_login_success(
                &state,
                &result.user_info.tenant_id,
                &result.user_info.username,
                &ip,
                ua,
            )
            .await;
            Ok((
                jar.add(refresh_cookie(
                    &result.refresh_token,
                    result.refresh_expires_at,
                )),
                Json(ApiResponse::success(LoginResponse::from(result))),
            )
                .into_response())
        }
        Err(e) => {
            if matches!(e, AppError::ServiceUnavailable(_)) {
                ryframe_middleware::metrics::record_redis_degraded("refresh_session");
            }
            record_login_failure_log(&state, &claims.tenant_id, "unknown", &ip, ua, &e).await;
            let clear_cookie = matches!(e, AppError::Authentication(_));
            let concurrent = matches!(&e, AppError::Conflict(message) if message == "refresh already in progress");
            if matches!(&e, AppError::Authentication(message) if message.contains("replay detected"))
            {
                ryframe_middleware::metrics::record_refresh_replay();
            }
            let mut response = if clear_cookie {
                (clear_auth_cookies(jar), e).into_response()
            } else {
                e.into_response()
            };
            if concurrent {
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, HeaderValue::from_static("5"));
            }
            Ok(response)
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

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SECRET: &str = "logout-csrf-unit-test-secret";

    fn csrf_request(sid: Option<&str>) -> (CookieJar, HeaderMap) {
        let token = ryframe_auth::jwt::encode_csrf(TEST_SECRET, sid, CSRF_TTL_SECONDS).unwrap();
        let jar = CookieJar::new().add(csrf_cookie(&token));
        let mut headers = HeaderMap::new();
        headers.insert(CSRF_HEADER, token.parse().unwrap());
        (jar, headers)
    }

    #[test]
    fn csrf_is_required_even_without_a_refresh_session() {
        let error = verify_csrf(&CookieJar::new(), &HeaderMap::new(), TEST_SECRET, None)
            .expect_err("logout without a challenge must fail");
        assert!(matches!(error, AppError::Authorization(_)));
    }

    #[test]
    fn unbound_csrf_allows_an_idempotent_logout_without_refresh_cookie() {
        let (jar, headers) = csrf_request(None);
        let attempt_id = verify_csrf(&jar, &headers, TEST_SECRET, None).unwrap();
        let challenge = headers.get(CSRF_HEADER).unwrap().to_str().unwrap();
        let claims = ryframe_auth::jwt::decode_csrf(challenge, TEST_SECRET).unwrap();
        assert_eq!(attempt_id, claims.jti);
    }

    #[test]
    fn csrf_cannot_cross_refresh_families() {
        let (jar, headers) = csrf_request(Some("sid-a"));
        let error = verify_csrf(&jar, &headers, TEST_SECRET, Some("sid-b"))
            .expect_err("a challenge must be bound to the current refresh family");
        assert!(matches!(error, AppError::Authorization(_)));
    }

    #[test]
    fn production_refresh_and_csrf_cookies_are_secure() {
        let absolute_exp = chrono::Utc::now().timestamp() as usize + 600;
        let refresh = refresh_cookie_for_environment("signed-refresh", absolute_exp, Some("prod"));
        assert_eq!(
            refresh.expires_datetime().unwrap().unix_timestamp(),
            absolute_exp as i64
        );
        let cookies = [
            refresh,
            csrf_cookie_for_environment("signed-csrf", Some("prod")),
        ];
        for cookie in cookies {
            assert_eq!(
                cookie.secure(),
                Some(true),
                "{} must be Secure in prod",
                cookie.name()
            );
            assert_eq!(cookie.http_only(), Some(true));
        }

        assert!(secure_cookies_for_environment(Some("production")));
        assert!(!secure_cookies_for_environment(Some("dev")));
        assert!(!secure_cookies_for_environment(None));
    }
}
