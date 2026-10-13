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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricted_session_only_allows_password_context_and_logout() {
        for (method, path) in [
            (Method::GET, "/api/v1/auth/context"),
            (Method::PUT, "/api/v1/auth/profile/password"),
            (Method::POST, "/api/v1/auth/logout"),
        ] {
            assert!(allowed(&method, path));
        }
        for (method, path) in [
            (Method::GET, "/api/v1/system/users"),
            (Method::GET, "/api/v1/auth/profile"),
            (Method::PUT, "/api/v1/auth/profile"),
            (Method::POST, "/api/v1/message/ws-ticket"),
            (Method::GET, "/api/v1/auth/context/extra"),
            (Method::POST, "/api/v1/auth/profile/password"),
            (Method::GET, "/api/v1/auth/logout"),
        ] {
            assert!(!allowed(&method, path));
        }
    }
}
