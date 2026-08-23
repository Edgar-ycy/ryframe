use chrono::{DateTime, Utc};

pub const FILE_DEL_FLAG_NORMAL: &str = "0";
pub const FILE_UPLOAD_STATUS_PENDING: &str = "pending";
pub const FILE_UPLOAD_STATUS_CLEANUP: &str = "cleanup";
pub const FILE_UPLOAD_STATUS_READY: &str = "ready";

#[derive(Debug, Eq, PartialEq)]
pub struct FileCleanupRecord {
    pub id: i64,
    pub tenant_id: String,
    pub bucket: String,
    pub storage_path: String,
    pub upload_status: String,
    pub reservation_token: Option<String>,
    pub reservation_expires_at: Option<DateTime<Utc>>,
    pub del_flag: String,
}

/// 内部文件清理声明所使用的控制库事务。
#[async_trait::async_trait]
pub trait FileCleanupTransaction: crate::PersistenceTransaction + Send + Sync {
    async fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<()>;

    async fn find_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> ryframe_kernel::AppResult<Option<FileCleanupRecord>>;

    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn claim_expired_import<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        claim_token: &'a str,
        expired_before: DateTime<Utc>,
        claim_until: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn mark_unreferenced_config_package<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        now: DateTime<Utc>,
        cleanup_after: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;
}

/// 内部文件清理所需的持久化端口。
#[async_trait::async_trait]
pub trait FileCleanupPersistencePort: Send + Sync {
    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn FileCleanupTransaction>>;

    async fn find<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> ryframe_kernel::AppResult<Option<FileCleanupRecord>>;

    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn find_stale_config_packages(
        &self,
        ready_before: DateTime<Utc>,
        limit: u64,
    ) -> ryframe_kernel::AppResult<Vec<FileCleanupRecord>>;

    async fn find_expired_reservations(
        &self,
        now: DateTime<Utc>,
        limit: u64,
    ) -> ryframe_kernel::AppResult<Vec<FileCleanupRecord>>;

    async fn begin_expired_cleanup<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        now: DateTime<Utc>,
        cleanup_after: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn claim_expired_cleanup<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        claim_token: &'a str,
        claimed_at: DateTime<Utc>,
        claim_until: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn begin_owned_cleanup<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        reservation_token: &'a str,
        cleanup_after: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn defer_claim<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        claim_token: &'a str,
        updated_at: DateTime<Utc>,
        retry_at: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn complete_claim<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        claim_token: &'a str,
    ) -> ryframe_kernel::AppResult<bool>;
}
