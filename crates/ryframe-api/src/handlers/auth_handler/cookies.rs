use crate::http::{HttpResult, api_path};
use axum::http::HeaderMap;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use ryframe_auth::jwt::{Claims, TokenSettings};
use ryframe_kernel::AppError;

pub(super) const REFRESH_COOKIE: &str = "ryframe_refresh_token";
pub(super) const CSRF_COOKIE: &str = "ryframe_csrf";
pub(super) const CSRF_HEADER: &str = "x-csrf-token";
pub(super) const CSRF_TTL_SECONDS: usize = 300;

fn auth_cookie(
    name: &'static str,
    value: String,
    max_age_seconds: i64,
    secure: bool,
) -> Cookie<'static> {
    let max_age = cookie::time::Duration::seconds(max_age_seconds);
    Cookie::build((name, value))
        .path(api_path("auth"))
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .max_age(max_age)
        .expires(cookie::time::OffsetDateTime::now_utc().saturating_add(max_age))
        .build()
}

pub(super) fn refresh_cookie(token: &str, absolute_exp: usize, secure: bool) -> Cookie<'static> {
    let now = chrono::Utc::now().timestamp().max(0) as usize;
    let max_age = absolute_exp.saturating_sub(now).min(7 * 24 * 60 * 60) as i64;
    let mut cookie = auth_cookie(REFRESH_COOKIE, token.to_owned(), max_age, secure);
    if let Ok(timestamp) = i64::try_from(absolute_exp)
        && let Ok(expires) = cookie::time::OffsetDateTime::from_unix_timestamp(timestamp)
    {
        cookie.set_expires(expires);
    }
    cookie
}

pub(super) fn csrf_cookie(token: &str, secure: bool) -> Cookie<'static> {
    auth_cookie(
        CSRF_COOKIE,
        token.to_owned(),
        CSRF_TTL_SECONDS as i64,
        secure,
    )
}

fn removal_cookie(name: &'static str, secure: bool) -> Cookie<'static> {
    Cookie::build((name, ""))
        .path(api_path("auth"))
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .removal()
        .build()
}

pub(super) fn clear_auth_cookies(jar: CookieJar, secure: bool) -> CookieJar {
    jar.add(removal_cookie(REFRESH_COOKIE, secure))
        .add(removal_cookie(CSRF_COOKIE, secure))
}

pub(super) fn decode_refresh_cookie(
    jar: &CookieJar,
    settings: &TokenSettings,
) -> HttpResult<Claims> {
    let token = jar
        .get(REFRESH_COOKIE)
        .map(Cookie::value)
        .ok_or_else(|| AppError::Authentication("missing refresh cookie".into()))?;
    let claims = ryframe_auth::jwt::decode_token(token, settings)?;
    if claims.token_type != "refresh" || claims.sid.is_empty() {
        return Err(AppError::Authentication("invalid refresh cookie".into()).into());
    }
    Ok(claims)
}

pub(super) fn refresh_cookie_session_id(
    jar: &CookieJar,
    settings: &TokenSettings,
) -> Option<String> {
    jar.get(REFRESH_COOKIE)
        .and_then(|cookie| ryframe_auth::jwt::decode_token(cookie.value(), settings).ok())
        .filter(|claims| claims.token_type == "refresh" && !claims.sid.is_empty())
        .map(|claims| claims.sid)
}

pub(super) fn refresh_cookie_value(jar: &CookieJar) -> Option<&str> {
    jar.get(REFRESH_COOKIE).map(Cookie::value)
}

pub(super) fn csrf_header(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
}
