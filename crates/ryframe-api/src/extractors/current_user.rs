use axum::{extract::FromRequestParts, http::request::Parts};
use ryframe_auth::jwt::Claims;
use ryframe_common::{AppError, annotations::data_scope::DataScope};

/// 当前登录用户上下文
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub user_id: i64,
    pub username: String,
    pub roles: Vec<String>,
    pub permissions: Vec<String>,
    pub dept_id: Option<i64>,
    pub dept_path: Option<String>,
    pub data_scope: DataScope,
    pub is_super_admin: bool,
}

impl<S: Send + Sync> FromRequestParts<S> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, AppError> {
        // 取 Claims（认证中间件已注入）
        let _claims = parts
            .extensions
            .get::<Claims>()
            .ok_or_else(|| AppError::Authentication("未认证".into()))?;

        // 取已注入的 CurrentUser 缓存（认证中间件预查时缓存）
        if let Some(cached) = parts.extensions.get::<CurrentUser>() {
            return Ok(cached.clone());
        }

        Err(AppError::Internal(
            "CurrentUser 未注入到 request extensions".into(),
        ))
    }
}
