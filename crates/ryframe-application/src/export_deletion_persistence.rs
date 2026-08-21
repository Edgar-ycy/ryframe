use chrono::{DateTime, Utc};

use crate::{EnqueueJob, PersistenceFuture};

/// 导出记录整批删除受理所需的控制库事务。
pub trait ExportDeletionTransaction: Send + Sync {
    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn mark_delete_pending<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        ids: &'a [i64],
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, u64>;

    fn enqueue_cleanup(&self, command: EnqueueJob, now: DateTime<Utc>)
    -> PersistenceFuture<'_, ()>;

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

pub trait ExportDeletionPersistencePort: Send + Sync {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ExportDeletionTransaction>>;
}
