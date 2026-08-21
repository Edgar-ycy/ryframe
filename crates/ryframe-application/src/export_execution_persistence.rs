use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::PersistenceFuture;

#[derive(Debug)]
pub struct ExportExecutionRecord {
    pub id: i64,
    pub tenant_id: String,
    pub requester_id: i64,
    pub resource: String,
    pub request_params: Value,
    pub request_version: i32,
    pub permission_code: String,
    pub authorization_fingerprint: String,
    pub snapshot_at: DateTime<Utc>,
    pub upper_id: i64,
    pub matched_rows: i64,
    pub status: String,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ExportExecutionState {
    pub status: String,
    pub delete_pending_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ExportBackgroundLease {
    pub status: String,
    pub lease_owner: Option<String>,
    pub lease_until: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportStartDecision {
    Started,
    AlreadyRunning,
    ConcurrencyLimited,
    NotRunnable,
}

pub trait ExportExecutionTransaction: Send + Sync {
    fn try_start<'a>(
        &'a self,
        export_id: i64,
        tenant_id: &'a str,
        maximum_running: u64,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, ExportStartDecision>;

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

/// Worker 执行导出时使用的控制库状态端口。
pub trait ExportExecutionPersistencePort: Send + Sync {
    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn find_by_background_job(
        &self,
        background_job_id: i64,
    ) -> PersistenceFuture<'_, Option<ExportExecutionRecord>>;

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ExportExecutionTransaction>>;

    fn update_exported_rows(
        &self,
        export_id: i64,
        exported_rows: i64,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'_, bool>;

    fn find_background_lease(
        &self,
        background_job_id: i64,
    ) -> PersistenceFuture<'_, Option<ExportBackgroundLease>>;

    fn find_export_state(
        &self,
        export_id: i64,
    ) -> PersistenceFuture<'_, Option<ExportExecutionState>>;
}
