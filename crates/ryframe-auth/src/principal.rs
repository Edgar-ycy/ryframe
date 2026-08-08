use std::ops::Deref;

use async_trait::async_trait;
use axum::{extract::FromRequestParts, http::request::Parts};
use ryframe_http::HttpAppError;
use ryframe_kernel::AppError;
use ryframe_kernel::{ActorContext, AppResult};
use serde::{Deserialize, Serialize};

use crate::jwt::Claims;

/// 当前请求中一次解析完成的不可变认证身份。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestPrincipal {
    pub actor: ActorContext,
    /// 解析当前主体时使用的租户授权纪元，用于客户端检测授权变化。
    #[serde(default)]
    pub tenant_authorization_epoch: i32,
    /// 用户保存的语言偏好；请求头未指定可用语言时作为回退。
    pub preferred_locale: Option<String>,
    pub roles: Vec<String>,
    pub role_ids: Vec<i64>,
    pub permissions: Vec<String>,
    pub tenant_request_limit_per_minute: u32,
}

#[async_trait]
pub trait PrincipalResolver: Send + Sync {
    async fn resolve_principal(&self, claims: &Claims) -> AppResult<RequestPrincipal>;
}

impl Deref for RequestPrincipal {
    type Target = ActorContext;

    fn deref(&self) -> &Self::Target {
        &self.actor
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
