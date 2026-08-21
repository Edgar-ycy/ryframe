use chrono::{DateTime, Utc};

use crate::PersistenceFuture;

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
pub trait FileCleanupTransaction: Send + Sync {
    fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()>;

    fn find_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> PersistenceFuture<'a, Option<FileCleanupRecord>>;

    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn claim_expired_import<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        claim_token: &'a str,
        expired_before: DateTime<Utc>,
        claim_until: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn mark_unreferenced_config_package<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        now: DateTime<Utc>,
        cleanup_after: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

/// 内部文件清理所需的持久化端口。
pub trait FileCleanupPersistencePort: Send + Sync {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn FileCleanupTransaction>>;

    fn find<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> PersistenceFuture<'a, Option<FileCleanupRecord>>;

    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn find_stale_config_packages(
        &self,
        ready_before: DateTime<Utc>,
        limit: u64,
    ) -> PersistenceFuture<'_, Vec<FileCleanupRecord>>;

    fn find_expired_reservations(
        &self,
        now: DateTime<Utc>,
        limit: u64,
    ) -> PersistenceFuture<'_, Vec<FileCleanupRecord>>;

    fn begin_expired_cleanup<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        now: DateTime<Utc>,
        cleanup_after: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn claim_expired_cleanup<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        claim_token: &'a str,
        claimed_at: DateTime<Utc>,
        claim_until: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn begin_owned_cleanup<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        reservation_token: &'a str,
        cleanup_after: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn defer_claim<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        claim_token: &'a str,
        updated_at: DateTime<Utc>,
        retry_at: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn complete_claim<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        claim_token: &'a str,
    ) -> PersistenceFuture<'a, bool>;
}
