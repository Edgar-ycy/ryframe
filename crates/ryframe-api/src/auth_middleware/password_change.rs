use axum::http::Method;

/// 改密前只开放完整匹配的必要会话操作，避免前缀白名单放开普通接口。
pub(super) fn allowed(method: &Method, path: &str) -> bool {
    matches!(
        (method, path.trim_end_matches('/')),
        (&Method::GET, "/api/v1/auth/context")
            | (&Method::PUT, "/api/v1/auth/profile/password")
            | (&Method::POST, "/api/v1/auth/logout")
    )
}
