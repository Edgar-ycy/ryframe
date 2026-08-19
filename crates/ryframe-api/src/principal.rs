use std::{ops::Deref, sync::Arc};

use axum::{extract::FromRequestParts, http::request::Parts};
use ryframe_auth::RequestPrincipal as AuthPrincipal;
use ryframe_http::HttpAppError;
use ryframe_kernel::AppError;

/// Axum 当前主体提取器。
///
/// 请求中只保存一份认证主体；提取器克隆 `Arc`，不会复制角色、权限和数据范围集合。
#[derive(Clone)]
pub struct RequestPrincipal(Arc<AuthPrincipal>);

impl RequestPrincipal {
    pub(crate) const fn new(principal: Arc<AuthPrincipal>) -> Self {
        Self(principal)
    }

    pub fn shared(&self) -> &Arc<AuthPrincipal> {
        &self.0
    }
}

impl Deref for RequestPrincipal {
    type Target = AuthPrincipal;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl<S: Send + Sync> FromRequestParts<S> for RequestPrincipal {
    type Rejection = HttpAppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Self>()
            .cloned()
            .ok_or_else(|| AppError::Authentication("未认证".into()).into())
    }
}
