use chrono::{DateTime, Utc};
use ryframe_kernel::{ActorContext, ExportQuerySnapshot};
use serde_json::Value;

use crate::{EnqueueJob, system::ExportSelection};

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
#[async_trait::async_trait]
pub trait ExportRequestTransaction: Send + Sync {
    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn find_active<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        request_fingerprint: &'a str,
    ) -> ryframe_kernel::AppResult<Option<ExportRequesterRecord>>;

    async fn summarize_selection<'a>(
        &'a self,
        tenant_id: &'a str,
        actor: &'a ActorContext,
        selection: &'a ExportSelection,
    ) -> ryframe_kernel::AppResult<ExportQuerySnapshot>;

    async fn enqueue_job(
        &self,
        command: EnqueueJob,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<i64>;

    async fn create_export(
        &self,
        command: CreateExportRecord,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<ExportRequesterRecord>;

    async fn commit(self: Box<Self>) -> ryframe_kernel::AppResult<()>;

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()>;
}

#[async_trait::async_trait]
pub trait ExportRequestPersistencePort: Send + Sync {
    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn ExportRequestTransaction>>;
}
