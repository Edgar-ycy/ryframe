use std::sync::Arc;

/// 后台执行器允许领取和维护的租户范围。
///
/// 固定租户模式仍允许平台维护记录；具体数据库条件由持久化适配器生成。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExecutionTenantScope {
    tenant_id: Option<Arc<str>>,
}

impl ExecutionTenantScope {
    pub const fn all() -> Self {
        Self { tenant_id: None }
    }

    pub fn tenant_and_platform(tenant_id: impl Into<Arc<str>>) -> Self {
        Self {
            tenant_id: Some(tenant_id.into()),
        }
    }

    pub fn tenant_id(&self) -> Option<&str> {
        self.tenant_id.as_deref()
    }
}
