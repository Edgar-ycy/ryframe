use chrono::{DateTime, Utc};

#[derive(Debug, Eq, PartialEq)]
pub struct ExportArtifactState {
    pub status: String,
    pub result_file_id: Option<i64>,
}

#[derive(Debug)]
pub struct ExportArtifactFileDraft {
    pub id: i64,
    pub file_name: String,
    pub storage_path: String,
    pub bucket: String,
    pub file_url: String,
    pub file_size: i64,
    pub content_type: String,
    pub sha256: String,
    pub uploaded_by: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ExportArtifactFileRecord {
    pub id: i64,
    pub file_name: String,
    pub content_type: String,
    pub file_size: i64,
}

#[derive(Debug)]
pub struct CompleteExportArtifact {
    pub export_id: i64,
    pub file_id: i64,
    pub file_name: String,
    pub content_type: String,
    pub file_size: i64,
    pub expires_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
}

/// 导出结果落账所需的控制库事务。
#[async_trait::async_trait]
pub trait ExportArtifactTransaction: crate::PersistenceTransaction + Send + Sync {
    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn lock_export(
        &self,
        export_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ExportArtifactState>>;

    async fn insert_ready_file<'a>(
        &'a self,
        tenant_id: &'a str,
        file: ExportArtifactFileDraft,
    ) -> ryframe_kernel::AppResult<ExportArtifactFileRecord>;

    async fn mark_succeeded(
        &self,
        command: CompleteExportArtifact,
    ) -> ryframe_kernel::AppResult<bool>;
}

#[async_trait::async_trait]
pub trait ExportArtifactPersistencePort: Send + Sync {
    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn ExportArtifactTransaction>>;
}
