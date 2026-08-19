use async_trait::async_trait;
use ryframe_auth::{RequestPrincipal, jwt::Claims};
use ryframe_kernel::AppResult;

/// 根据已验证令牌解析当前授权主体的应用端口。
#[async_trait]
pub trait PrincipalResolver: Send + Sync {
    async fn resolve_principal(&self, claims: &Claims) -> AppResult<RequestPrincipal>;
}
