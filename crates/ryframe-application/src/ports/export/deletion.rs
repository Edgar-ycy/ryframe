use chrono::{DateTime, Utc};

use crate::EnqueueJob;

/// 导出记录整批删除受理所需的控制库事务。
#[async_trait::async_trait]
pub trait ExportDeletionTransaction: Send + Sync {
    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn mark_delete_pending<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        ids: &'a [i64],
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<u64>;

    async fn enqueue_cleanup(
        &self,
        command: EnqueueJob,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<()>;

    async fn commit(self: Box<Self>) -> ryframe_kernel::AppResult<()>;

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()>;
}

#[async_trait::async_trait]
pub trait ExportDeletionPersistencePort: Send + Sync {
    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn ExportDeletionTransaction>>;
}
