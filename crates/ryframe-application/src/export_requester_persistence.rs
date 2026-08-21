use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::PersistenceFuture;

#[derive(Debug)]
pub struct ExportRequesterRecord {
    pub id: i64,
    pub resource: String,
    pub status: String,
    pub result_file_name: Option<String>,
    pub content_type: Option<String>,
    pub file_size: Option<i64>,
    pub expires_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
    pub snapshot_at: DateTime<Utc>,
    pub matched_rows: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub notification_read_at: Option<DateTime<Utc>>,
    pub permission_code: String,
    pub request_params: Value,
    pub request_version: i32,
    pub authorization_fingerprint: String,
    pub upper_id: i64,
    pub result_file_id: Option<i64>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ExportDownloadFile {
    pub bucket: String,
    pub storage_path: String,
}

/// 申请人取消导出时使用的控制库事务。
pub trait ExportRequesterTransaction: Send + Sync {
    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn cancel<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        export_id: i64,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

/// 面向导出申请人的查询与状态变更端口。
pub trait ExportRequesterPersistencePort: Send + Sync {
    fn find<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        export_id: i64,
    ) -> PersistenceFuture<'a, Option<ExportRequesterRecord>>;

    fn list_recent<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        limit: u64,
    ) -> PersistenceFuture<'a, Vec<ExportRequesterRecord>>;

    fn list_recent_for_notifications<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        limit: u64,
    ) -> PersistenceFuture<'a, Vec<ExportRequesterRecord>>;

    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn mark_notifications_read<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        ids: &'a [i64],
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, u64>;

    fn find_download_file<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> PersistenceFuture<'a, Option<ExportDownloadFile>>;

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ExportRequesterTransaction>>;
}
