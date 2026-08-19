//! 请求范围租户身份的一致性校验。
//!
//! 数据放置和目标选择由 `ryframe-tenant-db` 的权威 placement/fence 完成；本模块不再
//! 暴露无实际路由能力的隔离策略或 Repository 包装器。

use std::future::Future;

use ryframe_kernel::{AppError, AppResult};

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
pub fn validate_explicit_tenant(tenant_id: &str) -> AppResult<()> {
    validate_tenant_identifier(tenant_id)?;
    REQUEST_TENANT_CONTEXT
        .try_with(|context| {
            if context.tenant_id == tenant_id {
                Ok(())
            } else {
                Err(AppError::Authorization("请求租户与业务租户不一致".into()))
            }
        })
        .unwrap_or(Ok(()))
}

/// 标识符用于数据库分区、缓存键或 Redis 通配模式前必须先经过该校验。
pub fn validate_tenant_identifier(tenant_id: &str) -> AppResult<()> {
    let bytes = tenant_id.as_bytes();
    let is_alphanumeric = |byte: u8| byte.is_ascii_alphanumeric();
    if !(2..=64).contains(&bytes.len())
        || !bytes.first().is_some_and(|byte| is_alphanumeric(*byte))
        || !bytes.last().is_some_and(|byte| is_alphanumeric(*byte))
        || !bytes
            .iter()
            .all(|byte| is_alphanumeric(*byte) || matches!(byte, b'-' | b'_'))
    {
        return Err(AppError::Validation(
            "tenant ID must be 2-64 ASCII letters, digits, hyphens, or underscores and start/end with a letter or digit"
                .into(),
        ));
    }
    Ok(())
}

/// 在显式租户范围内运行异步任务，供认证中间件安装一致性校验上下文。
pub async fn with_tenant_context<F>(context: TenantContext, future: F) -> F::Output
where
    F: Future,
{
    REQUEST_TENANT_CONTEXT.scope(context, future).await
}
