use sea_orm::{ColumnTrait, Condition};

/// 后台执行器允许领取和维护的租户范围。
///
/// `tenant_id = NULL` 表示不属于任一业务租户的平台维护记录，因此固定租户范围始终
/// 同时保留平台记录。范围在数据库查询和加锁前应用，不能在领取后再做拒绝判断。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExecutionTenantScope {
    tenant_id: Option<String>,
}

impl ExecutionTenantScope {
    /// 允许处理全部租户以及平台级记录。
    pub const fn all() -> Self {
        Self { tenant_id: None }
    }

    /// 只允许处理指定租户以及平台级记录。
    pub fn tenant_and_platform(tenant_id: impl Into<String>) -> Self {
        Self {
            tenant_id: Some(tenant_id.into()),
        }
    }

    /// 返回固定业务租户；`None` 表示允许全部租户。
    pub fn tenant_id(&self) -> Option<&str> {
        self.tenant_id.as_deref()
    }

    /// 为可空租户列构造执行范围条件；全租户范围无需附加条件。
    pub(crate) fn condition<C>(&self, column: C) -> Option<Condition>
    where
        C: ColumnTrait,
    {
        self.tenant_id().map(|tenant_id| {
            Condition::any()
                .add(column.eq(tenant_id))
                .add(column.is_null())
        })
    }
}
