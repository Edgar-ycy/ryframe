use chrono::{DateTime, Utc};

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

#[async_trait::async_trait]
pub trait ExportCleanupTransaction: crate::PersistenceTransaction + Send + Sync {
    async fn lock_export(
        &self,
        export_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ExportCleanupRecord>>;

    async fn hard_delete_file<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn delete_pending_export(&self, export_id: i64) -> ryframe_kernel::AppResult<bool>;

    async fn mark_expired(
        &self,
        export_id: i64,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;
}

/// 导出墓碑与过期结果清理使用的控制库端口。
#[async_trait::async_trait]
pub trait ExportCleanupPersistencePort: Send + Sync {
    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn list_delete_pending(
        &self,
        after_id: Option<i64>,
        limit: u64,
    ) -> ryframe_kernel::AppResult<Vec<ExportCleanupRecord>>;

    async fn list_expired(
        &self,
        now: DateTime<Utc>,
        after_id: Option<i64>,
        limit: u64,
    ) -> ryframe_kernel::AppResult<Vec<ExportCleanupRecord>>;

    async fn lookup_result_file<'a>(
        &'a self,
        tenant_id: &'a str,
        export_id: i64,
        file_id: i64,
    ) -> ryframe_kernel::AppResult<ExportCleanupFileLookup>;

    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn ExportCleanupTransaction>>;
}
