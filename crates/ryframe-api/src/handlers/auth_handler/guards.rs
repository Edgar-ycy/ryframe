use std::net::SocketAddr;

use crate::http::{ApiResponse, HttpResult, api_path};
use axum::{
    Json,
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use ryframe_application::TenantContext;
use ryframe_auth::jwt::TokenSettings;
use ryframe_kernel::AppError;

use super::cookies::{CSRF_COOKIE, csrf_header};
use crate::{dto::auth_dto::LoginRequest, state::AppState};

pub(super) fn extract_ip(state: &AppState, headers: &HeaderMap, remote_addr: SocketAddr) -> String {
    state
        .trusted_proxies
        .client_ip(headers, remote_addr.ip())
        .to_string()
}

pub(super) fn extract_user_agent(headers: &HeaderMap) -> &str {
    headers
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
}

pub(super) fn verify_csrf(
    jar: &CookieJar,
    headers: &HeaderMap,
    settings: &TokenSettings,
    expected_sid: Option<&str>,
) -> HttpResult<String> {
    let header_token = csrf_header(headers).ok_or_else(|| {
        crate::metrics::record_csrf_rejection();
        AppError::Authorization("missing CSRF challenge".into())
    })?;
    let cookie_token = jar.get(CSRF_COOKIE).map(Cookie::value).ok_or_else(|| {
        crate::metrics::record_csrf_rejection();
        AppError::Authorization("missing CSRF challenge cookie".into())
    })?;
    if header_token != cookie_token {
        crate::metrics::record_csrf_rejection();
        return Err(AppError::Authorization("CSRF challenge mismatch".into()).into());
    }
    let claims = ryframe_auth::jwt::decode_csrf(header_token, settings).inspect_err(|_| {
        crate::metrics::record_csrf_rejection();
    })?;
    if claims.sid.as_deref() != expected_sid {
        crate::metrics::record_csrf_rejection();
        return Err(
            AppError::Authorization("CSRF challenge is not bound to this session".into()).into(),
        );
    }
    Ok(claims.jti)
}

pub(super) fn validate_auth_origin(state: &AppState, headers: &HeaderMap) -> HttpResult<()> {
    match headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        Some(origin)
            if state
                .settings
                .cors
                .allow_origins
                .iter()
                .any(|allowed| allowed == origin) =>
        {
            Ok(())
        }
        Some(_) => {
            crate::metrics::record_csrf_rejection();
            Err(AppError::Authorization("request origin is not allowed".into()).into())
        }
        None if state.settings.production => {
            crate::metrics::record_csrf_rejection();
            Err(AppError::Authorization("Origin header is required in production".into()).into())
        }
        None => Ok(()),
    }
}

pub(super) async fn enforce_login_rate_limit(
    state: &AppState,
    tenant_id: &str,
    username: &str,
    client_ip: &str,
) -> HttpResult<Option<Response>> {
    if !state.settings.rate_limit.enabled {
        return Ok(None);
    }
    let login_rate_limit_rule = format!("POST {}", api_path("auth/login"));
    let limit = state
        .settings
        .rate_limit
        .api_limits
        .get(&login_rate_limit_rule)
        .copied()
        .unwrap_or(5);
    let window = state.settings.rate_limit.api_window_secs.max(1);
    let normalized_username = username.trim().to_lowercase();
    let principal_digest = ryframe_auth::stable_scope_digest(&[tenant_id, &normalized_username]);
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
                crate::metrics::record_redis_degraded("login_rate_limit");
                AppError::ServiceUnavailable("rate limit service unavailable".into())
            })?;
        if !decision.allowed {
            crate::metrics::record_rate_limit_rejection(scope);
            let mut response = (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                Json(ApiResponse::<()>::fail(
                    429,
                    "too many login attempts; try again later",
                    "rate_limited",
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

pub(super) async fn verify_captcha_if_enabled(
    state: &AppState,
    tenant_id: &str,
    req: &LoginRequest,
) -> HttpResult<()> {
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
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::Validation("验证码ID不能为空".into()))?;
    let captcha_code = req
        .captcha_code
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::Validation("验证码不能为空".into()))?;
    let valid = state
        .services
        .captcha
        .verify(captcha_id, captcha_code)
        .await
        .inspect_err(|_| {
            crate::metrics::record_redis_degraded("captcha_store");
        })?;
    if !valid {
        return Err(AppError::Validation("验证码错误或已过期".into()).into());
    }
    Ok(())
}

pub(super) fn tenant_id(
    state: &AppState,
    tenant_context: Option<axum::Extension<TenantContext>>,
    headers: &HeaderMap,
) -> HttpResult<String> {
    if state.settings.multi_tenancy.fixed_tenant_id().is_some() {
        return crate::handler_utils::tenant_id_from_headers(
            headers,
            &state.settings.multi_tenancy,
        );
    }
    tenant_context
        .map(|axum::Extension(context)| context.tenant_id)
        .map(Ok)
        .unwrap_or_else(|| {
            crate::handler_utils::tenant_id_from_headers(headers, &state.settings.multi_tenancy)
        })
}
