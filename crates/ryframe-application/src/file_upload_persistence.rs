use chrono::{DateTime, Utc};

use crate::PersistenceFuture;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileUploadCommitMode {
    CurrentRequest,
    Unbound,
}

#[derive(Debug, Eq, PartialEq)]
pub struct FileUploadRecord {
    pub id: i64,
    pub tenant_id: String,
    pub original_name: String,
    pub storage_name: String,
    pub storage_path: String,
    pub bucket: String,
    pub file_url: String,
    pub file_size: i64,
    pub content_type: String,
    pub file_sha256: String,
    pub upload_by: Option<String>,
    pub upload_status: String,
    pub reservation_token: Option<String>,
    pub reservation_expires_at: Option<DateTime<Utc>>,
    pub del_flag: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 文件上传预留与完成状态所使用的控制库事务。
pub trait FileUploadTransaction: Send + Sync {
    fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()>;

    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn find_by_sha256_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        bucket: &'a str,
        file_sha256: &'a str,
    ) -> PersistenceFuture<'a, Option<FileUploadRecord>>;

    fn restore_for_reference<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        bucket: &'a str,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn ensure_storage_quota<'a>(
        &'a self,
        tenant_id: &'a str,
        additional_bytes: u64,
    ) -> PersistenceFuture<'a, ()>;

    fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: FileUploadRecord,
    ) -> PersistenceFuture<'a, FileUploadRecord>;

    fn mark_ready<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        reservation_token: &'a str,
        updated_at: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn commit(self: Box<Self>, mode: FileUploadCommitMode) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

/// 文件上传状态机所需的持久化端口。
pub trait FileUploadPersistencePort: Send + Sync {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn FileUploadTransaction>>;

    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn renew_pending<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        reservation_token: &'a str,
        expires_at: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn find_any<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> PersistenceFuture<'a, Option<FileUploadRecord>>;

    fn find_ready<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> PersistenceFuture<'a, Option<FileUploadRecord>>;
}
