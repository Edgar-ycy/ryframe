use chrono::{DateTime, Utc};
use ryframe_kernel::{ActorContext, ExportQuerySnapshot};
use serde_json::Value;

use crate::{EnqueueJob, PersistenceFuture, system::ExportSelection};

use super::ExportRequesterRecord;

#[derive(Debug)]
pub struct CreateExportRecord {
    pub tenant_id: String,
    pub requester_id: i64,
    pub resource: String,
    pub background_job_id: i64,
    pub request_params: Value,
    pub request_version: i32,
    pub permission_code: String,
    pub authorization_fingerprint: String,
    pub request_fingerprint: String,
    pub snapshot_at: DateTime<Utc>,
    pub upper_id: i64,
    pub matched_rows: i64,
}

/// 导出申请创建所需的控制库一致性事务。
pub trait ExportRequestTransaction: Send + Sync {
    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn find_active<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        request_fingerprint: &'a str,
    ) -> PersistenceFuture<'a, Option<ExportRequesterRecord>>;

    fn summarize_selection<'a>(
        &'a self,
        tenant_id: &'a str,
        actor: &'a ActorContext,
        selection: &'a ExportSelection,
    ) -> PersistenceFuture<'a, ExportQuerySnapshot>;

    fn enqueue_job(&self, command: EnqueueJob, now: DateTime<Utc>) -> PersistenceFuture<'_, i64>;

    fn create_export(
        &self,
        command: CreateExportRecord,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'_, ExportRequesterRecord>;

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

pub trait ExportRequestPersistencePort: Send + Sync {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ExportRequestTransaction>>;
}
