use serde::{Deserialize, Serialize};

pub use ryframe_kernel::DATA_SCOPE_PROJECTION_OUTBOX_EVENT_TYPE;

/// 身份或组织事实变更后需要发布的精确投影版本。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DataScopeProjectionEvent {
    pub tenant_id: String,
    pub source_version: i64,
}

/// 在控制库业务事务内推进数据范围版本并原子写入投影事件。
#[async_trait::async_trait]
pub trait DataScopeMutationTransaction: Send + Sync {
    async fn record_data_scope_change(&self, tenant_id: &str) -> ryframe_kernel::AppResult<i64>;
}

/// 将租户数据范围投影发布到当前 placement 目标。
#[async_trait::async_trait]
pub trait DataScopeProjectionPublisher: Send + Sync {
    async fn publish(&self, event: &DataScopeProjectionEvent) -> ryframe_kernel::AppResult<()>;

    async fn publish_for_migration(
        &self,
        fence: super::tenant_data::TenantDataFence<'_>,
    ) -> ryframe_kernel::AppResult<i64>;
}
