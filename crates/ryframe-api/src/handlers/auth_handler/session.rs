use std::net::SocketAddr;

use axum::{
    Extension, Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use ryframe_auth::{RequestPrincipal, jwt::Claims};
use ryframe_core::RefreshSessionRevocation;
use ryframe_http::{ApiResponse, HttpAppError, HttpResult};
use ryframe_kernel::AppError;

use super::{
    cookies::{
        CSRF_TTL_SECONDS, REFRESH_COOKIE, clear_auth_cookies, csrf_cookie, decode_refresh_cookie,
        refresh_cookie, refresh_cookie_session_id, refresh_cookie_value,
    },
    guards::{extract_ip, extract_user_agent, validate_auth_origin, verify_csrf},
    login::{record_login_failure_log, record_login_success},
};
use crate::{
    dto::{
        auth_dto::{AuthSessionResponse, CsrfResponse, LoginResponse, RevokeOtherSessionsResponse},
        public_dto::UserInfo,
    },
    state::AppState,
};

#[utoipa::path(
    get,
    path = "/api/v1/auth/csrf",
    tag = "认证",
    responses((status = 200, description = "CSRF 挑战令牌", body = ApiResponse<CsrfResponse>))
)]
/// 获取短期 CSRF 挑战令牌
///
/// GET /api/v1/auth/csrf
pub async fn csrf(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> HttpResult<Response> {
    validate_auth_origin(&state, &headers)?;
    let sid = refresh_cookie_session_id(&jar, &state.config.auth.jwt_secret);
    let token = ryframe_auth::jwt::encode_csrf(
        &state.config.auth.jwt_secret,
        sid.as_deref(),
        CSRF_TTL_SECONDS,
    )?;
    let mut response = (
        jar.add(csrf_cookie(&token, state.config.environment)),
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
    path = "/api/v1/auth/logout",
    tag = "认证",
    responses(
        (status = 200, description = "登出成功", body = ryframe_http::ApiEmptyResponse),
        (status = 403, description = "CSRF 挑战令牌缺失、无效或与会话不匹配"),
        (status = 503, description = "Redis 会话或撤销服务不可用"),
    ),
    params(("X-CSRF-Token" = String, Header, description = "已签名的挑战令牌；存在刷新 Cookie 时与 sid 绑定")),
    security((), ("refreshCookie" = []))
)]
/// 用户登出
///
/// POST /api/v1/auth/logout
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> HttpResult<Response> {
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
        Err(HttpAppError(AppError::Authentication(_))) if !has_refresh_cookie => None,
        Err(error) => {
            return Ok((clear_auth_cookies(jar, state.config.environment), error).into_response());
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
        if let Err(error) = state
            .services
            .online_user
            .remove_user(&claims.tenant_id, &claims.sid)
            .await
        {
            ryframe_middleware::metrics::record_redis_degraded("logout_session_metadata");
            tracing::warn!(%error, sid = %claims.sid, "清理退出会话元数据失败");
        }
    }
    Ok((
        clear_auth_cookies(jar, state.config.environment),
        Json(ApiResponse::<()>::success_no_data()),
    )
        .into_response())
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/refresh",
    params(("X-CSRF-Token" = String, Header, description = "与会话绑定的 CSRF 挑战令牌")),
    security(("refreshCookie" = [])),
    tag = "认证",
    responses(
        (status = 200, description = "刷新成功", body = ApiResponse<LoginResponse>),
        (status = 401, description = "令牌无效、已过期、被撤销或确认重放"),
        (status = 403, description = "CSRF 挑战令牌缺失、无效或与会话不匹配"),
        (status = 409, description = "另一个令牌轮换请求正在处理", headers(("Retry-After" = String, description = "再次刷新前等待的秒数"))),
        (status = 503, description = "Redis 会话服务不可用；显式重试必须复用原 X-CSRF-Token")
    )
)]
/// 刷新令牌
///
/// POST /api/v1/auth/refresh
pub async fn refresh(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
) -> HttpResult<Response> {
    validate_auth_origin(&state, &headers)?;
    let claims = match decode_refresh_cookie(&jar, &state.config.auth.jwt_secret) {
        Ok(claims) => claims,
        Err(error) => {
            return Ok((clear_auth_cookies(jar, state.config.environment), error).into_response());
        }
    };
    let rotation_attempt_id = verify_csrf(
        &jar,
        &headers,
        &state.config.auth.jwt_secret,
        Some(&claims.sid),
    )?;
    let refresh_token = refresh_cookie_value(&jar)
        .expect("刷新 Cookie 已在上方完成校验")
        .to_owned();
    let ip = extract_ip(&state, &headers, addr);
    let user_agent = extract_user_agent(&headers);

    match state
        .services
        .auth
        .refresh_token(&refresh_token, &rotation_attempt_id)
        .await
    {
        Ok(result) => {
            state
                .services
                .online_user
                .touch_user_strict(&result.user_info.tenant_id, &result.sid)
                .await
                .inspect_err(|_| {
                    ryframe_middleware::metrics::record_redis_degraded("refresh_session_metadata");
                })?;
            record_login_success(
                &state,
                &result.user_info.tenant_id,
                &result.user_info.username,
                &ip,
                user_agent,
            )
            .await;
            Ok((
                jar.add(refresh_cookie(
                    &result.refresh_token,
                    result.refresh_expires_at,
                    state.config.environment,
                )),
                Json(ApiResponse::success(LoginResponse::from(result))),
            )
                .into_response())
        }
        Err(error) => {
            if matches!(&error, AppError::ServiceUnavailable(_)) {
                ryframe_middleware::metrics::record_redis_degraded("refresh_session");
            }
            record_login_failure_log(
                &state,
                &claims.tenant_id,
                "unknown",
                &ip,
                user_agent,
                &error,
            )
            .await;
            let clear_cookie = matches!(&error, AppError::Authentication(_));
            let concurrent = matches!(&error, AppError::Conflict(message) if message == "refresh already in progress");
            if matches!(&error, AppError::Authentication(message) if message.contains("replay detected"))
            {
                ryframe_middleware::metrics::record_refresh_replay();
            }
            let error = HttpAppError::from(error);
            let mut response = if clear_cookie {
                (clear_auth_cookies(jar, state.config.environment), error).into_response()
            } else {
                error.into_response()
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
    get,
    path = "/api/v1/auth/me",
    tag = "认证",
    responses(
        (status = 200, description = "用户信息", body = ApiResponse<UserInfo>),
        (status = 401, description = "未认证")
    ),
    security(("bearer" = []))
)]
/// 获取当前用户信息
///
/// GET /api/v1/auth/me
pub async fn me(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> HttpResult<Json<ApiResponse<UserInfo>>> {
    let user_info = state.services.auth.get_current_user(&current_user).await?;

    Ok(Json(ApiResponse::success(user_info.into())))
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/sessions",
    tag = "认证",
    responses(
        (status = 200, description = "当前用户的登录设备", body = ApiResponse<Vec<AuthSessionResponse>>),
        (status = 401, description = "未认证"),
        (status = 503, description = "会话服务不可用")
    ),
    security(("bearer" = []))
)]
/// 查询当前用户的登录设备。
pub async fn list_sessions(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Extension(claims): Extension<Claims>,
) -> HttpResult<Json<ApiResponse<Vec<AuthSessionResponse>>>> {
    let mut sessions = state
        .services
        .online_user
        .list_user_sessions(&current_user.tenant_id, current_user.user_id)
        .await?;
    sessions.sort_by(|left, right| {
        let left_current = left.sid == claims.sid;
        let right_current = right.sid == claims.sid;
        right_current
            .cmp(&left_current)
            .then_with(|| right.last_access_time.cmp(&left.last_access_time))
            .then_with(|| left.sid.cmp(&right.sid))
    });
    let sessions = sessions
        .into_iter()
        .map(|session| {
            let expires_at =
                chrono::DateTime::<chrono::Utc>::from_timestamp(session.absolute_exp, 0)
                    .ok_or_else(|| {
                        tracing::error!(sid = %session.sid, "登录设备绝对过期时间超出可表示范围");
                        AppError::ServiceUnavailable("登录设备数据暂不可用".into())
                    })?;
            Ok(AuthSessionResponse {
                current: session.sid == claims.sid,
                sid: session.sid,
                ipaddr: session.ipaddr,
                login_location: session.login_location,
                browser: session.browser,
                os: session.os,
                login_time: session.login_time,
                last_access_time: session.last_access_time,
                expires_at,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(ApiResponse::success(sessions)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/auth/sessions/{sid}",
    tag = "认证",
    params(
        ("sid" = String, Path, description = "稳定的设备会话标识"),
        ("X-CSRF-Token" = String, Header, description = "与当前访问会话绑定的 CSRF 挑战令牌")
    ),
    responses(
        (status = 200, description = "会话已撤销", body = ryframe_http::ApiEmptyResponse),
        (status = 401, description = "未认证"),
        (status = 403, description = "CSRF 挑战令牌无效"),
        (status = 404, description = "会话不存在或不属于当前用户"),
        (status = 503, description = "会话服务不可用")
    ),
    security(("bearer" = []))
)]
/// 撤销当前用户的一台登录设备。
pub async fn revoke_session(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Extension(claims): Extension<Claims>,
    Path(sid): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> HttpResult<Response> {
    validate_auth_origin(&state, &headers)?;
    verify_csrf(
        &jar,
        &headers,
        &state.config.auth.jwt_secret,
        Some(&claims.sid),
    )?;
    let result = state
        .services
        .auth
        .refresh_sessions()
        .revoke_for_user(&current_user.tenant_id, current_user.user_id, &sid)
        .await
        .inspect_err(|_| {
            ryframe_middleware::metrics::record_redis_degraded("profile_session_revoke");
        })?;
    if matches!(result, RefreshSessionRevocation::NotFoundOrForeign) {
        return Err(AppError::NotFound("登录设备不存在".into()).into());
    }
    if let Err(error) = state
        .services
        .online_user
        .remove_user(&current_user.tenant_id, &sid)
        .await
    {
        ryframe_middleware::metrics::record_redis_degraded("profile_session_metadata_cleanup");
        tracing::warn!(%error, %sid, "撤销会话后的展示元数据清理失败");
    }

    let response = Json(ApiResponse::<()>::success_no_data());
    if sid == claims.sid {
        Ok((clear_auth_cookies(jar, state.config.environment), response).into_response())
    } else {
        Ok((jar, response).into_response())
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/sessions/revoke-others",
    tag = "认证",
    params(("X-CSRF-Token" = String, Header, description = "与当前访问会话绑定的 CSRF 挑战令牌")),
    responses(
        (status = 200, description = "其他会话已撤销", body = ApiResponse<RevokeOtherSessionsResponse>),
        (status = 400, description = "可治理的会话候选超过单次安全上限"),
        (status = 401, description = "未认证"),
        (status = 403, description = "CSRF 挑战令牌无效"),
        (status = 503, description = "会话服务不可用")
    ),
    security(("bearer" = []))
)]
/// 撤销当前用户除本设备之外的全部登录设备。
pub async fn revoke_other_sessions(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Extension(claims): Extension<Claims>,
    headers: HeaderMap,
    jar: CookieJar,
) -> HttpResult<Json<ApiResponse<RevokeOtherSessionsResponse>>> {
    validate_auth_origin(&state, &headers)?;
    verify_csrf(
        &jar,
        &headers,
        &state.config.auth.jwt_secret,
        Some(&claims.sid),
    )?;
    let sessions = state
        .services
        .online_user
        .list_user_sessions(&current_user.tenant_id, current_user.user_id)
        .await?;
    let mut candidate_sids = state
        .services
        .auth
        .refresh_sessions()
        .session_sids_for_user(&current_user.tenant_id, current_user.user_id)
        .await?;
    candidate_sids.extend(sessions.into_iter().map(|session| session.sid));
    candidate_sids.sort_unstable();
    candidate_sids.dedup();
    let revoked_count = state
        .services
        .auth
        .refresh_sessions()
        .revoke_other_sessions_for_user(
            &current_user.tenant_id,
            current_user.user_id,
            &claims.sid,
            &candidate_sids,
        )
        .await
        .inspect_err(|_| {
            ryframe_middleware::metrics::record_redis_degraded("profile_session_revoke_others");
        })?;

    // Refresh Family 已经完成权威批量撤销；展示元数据清理失败不能回滚或掩盖安全结果。
    for sid in candidate_sids.iter().filter(|sid| *sid != &claims.sid) {
        if let Err(error) = state
            .services
            .online_user
            .remove_user(&current_user.tenant_id, sid)
            .await
        {
            ryframe_middleware::metrics::record_redis_degraded("profile_session_metadata_cleanup");
            tracing::warn!(%error, %sid, "批量撤销后的展示元数据清理失败");
        }
    }
    Ok(Json(ApiResponse::success(RevokeOtherSessionsResponse {
        revoked_count,
    })))
}
