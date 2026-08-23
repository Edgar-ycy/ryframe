use chrono::{DateTime, Utc};

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
#[async_trait::async_trait]
pub trait FileUploadTransaction: crate::PersistenceTransaction + Send + Sync {
    async fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<()>;

    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn find_by_sha256_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        bucket: &'a str,
        file_sha256: &'a str,
    ) -> ryframe_kernel::AppResult<Option<FileUploadRecord>>;

    async fn restore_for_reference<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        bucket: &'a str,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn ensure_storage_quota<'a>(
        &'a self,
        tenant_id: &'a str,
        additional_bytes: u64,
    ) -> ryframe_kernel::AppResult<()>;

    async fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: FileUploadRecord,
    ) -> ryframe_kernel::AppResult<FileUploadRecord>;

    async fn mark_ready<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        reservation_token: &'a str,
        updated_at: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;
}

/// 文件上传状态机所需的持久化端口。
#[async_trait::async_trait]
pub trait FileUploadPersistencePort: Send + Sync {
    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn FileUploadTransaction>>;

    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn renew_pending<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        reservation_token: &'a str,
        expires_at: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn find_any<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> ryframe_kernel::AppResult<Option<FileUploadRecord>>;

    async fn find_ready<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> ryframe_kernel::AppResult<Option<FileUploadRecord>>;
}
