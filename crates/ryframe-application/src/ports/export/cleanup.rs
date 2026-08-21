use chrono::{DateTime, Utc};

use crate::PersistenceFuture;

#[derive(Debug)]
pub struct ExportCleanupRecord {
    pub id: i64,
    pub tenant_id: String,
    pub status: String,
    pub result_file_id: Option<i64>,
    pub expires_at: Option<DateTime<Utc>>,
    pub delete_pending_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ExportCleanupFile {
    pub id: i64,
    pub bucket: String,
    pub storage_path: String,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ExportCleanupFileLookup {
    ExportMissing,
    FileMissing,
    Found(ExportCleanupFile),
}

pub trait ExportCleanupTransaction: Send + Sync {
    fn lock_export(&self, export_id: i64) -> PersistenceFuture<'_, Option<ExportCleanupRecord>>;

    fn hard_delete_file<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> PersistenceFuture<'a, bool>;

    fn delete_pending_export(&self, export_id: i64) -> PersistenceFuture<'_, bool>;

    fn mark_expired(&self, export_id: i64, now: DateTime<Utc>) -> PersistenceFuture<'_, bool>;

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

/// 导出墓碑与过期结果清理使用的控制库端口。
pub trait ExportCleanupPersistencePort: Send + Sync {
    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn list_delete_pending(
        &self,
        after_id: Option<i64>,
        limit: u64,
    ) -> PersistenceFuture<'_, Vec<ExportCleanupRecord>>;

    fn list_expired(
        &self,
        now: DateTime<Utc>,
        after_id: Option<i64>,
        limit: u64,
    ) -> PersistenceFuture<'_, Vec<ExportCleanupRecord>>;

    fn lookup_result_file<'a>(
        &'a self,
        tenant_id: &'a str,
        export_id: i64,
        file_id: i64,
    ) -> PersistenceFuture<'a, ExportCleanupFileLookup>;

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ExportCleanupTransaction>>;
}
