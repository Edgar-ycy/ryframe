//! 请求范围租户身份的一致性校验。
//!
//! 数据放置和目标选择由 `ryframe-tenant-db` 的权威 placement/fence 完成；本模块不再
//! 暴露无实际路由能力的隔离策略或 Repository 包装器。

use std::future::Future;

use ryframe_kernel::{AppError, AppResult, TenantId};

tokio::task_local! {
    /// 仅用于核验显式用例输入的请求范围租户身份。
    static REQUEST_TENANT_CONTEXT: TenantContext;
}

#[derive(Debug, Clone)]
pub struct TenantContext {
    pub tenant_id: String,
    pub is_admin: bool,
}

impl TenantContext {
    pub fn admin() -> Self {
        Self {
            tenant_id: "system".into(),
            is_admin: true,
        }
    }
}

/// 存在请求本地状态时，核验显式用例租户是否与其一致。后台任务没有请求本地状态，
/// 其显式租户输入仍是权威范围。
pub fn enforce_tenant_context(tenant_id: TenantId<'_>) -> AppResult<()> {
    REQUEST_TENANT_CONTEXT
        .try_with(|context| {
            if context.tenant_id == tenant_id.as_str() {
                Ok(())
            } else {
                Err(AppError::Authorization("请求租户与业务租户不一致".into()))
            }
        })
        .unwrap_or(Ok(()))
}

/// 在显式租户范围内运行异步任务，供认证中间件安装一致性校验上下文。
pub async fn with_tenant_context<F>(context: TenantContext, future: F) -> F::Output
where
    F: Future,
{
    REQUEST_TENANT_CONTEXT.scope(context, future).await
}
