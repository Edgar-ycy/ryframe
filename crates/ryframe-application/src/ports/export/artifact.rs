use chrono::{DateTime, Utc};

use crate::PersistenceFuture;

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
pub trait ExportArtifactTransaction: Send + Sync {
    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn lock_export(&self, export_id: i64) -> PersistenceFuture<'_, Option<ExportArtifactState>>;

    fn insert_ready_file<'a>(
        &'a self,
        tenant_id: &'a str,
        file: ExportArtifactFileDraft,
    ) -> PersistenceFuture<'a, ExportArtifactFileRecord>;

    fn mark_succeeded(&self, command: CompleteExportArtifact) -> PersistenceFuture<'_, bool>;

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

pub trait ExportArtifactPersistencePort: Send + Sync {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ExportArtifactTransaction>>;
}
