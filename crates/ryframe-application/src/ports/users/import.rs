use chrono::{DateTime, Utc};
use ryframe_kernel::{PageResult, ValidatedPageQuery};

use crate::ports::jobs::BackgroundJobTransaction;

#[derive(Debug)]
pub struct UserImportDepartmentRecord {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub ancestors: String,
    pub status: String,
}

impl UserImportDepartmentRecord {
    const STATUS_NORMAL: &'static str = "1";

    pub fn is_enabled(&self) -> bool {
        self.status == Self::STATUS_NORMAL
    }
}

#[derive(Debug)]
pub struct UserImportJobRecord {
    pub id: i64,
    pub tenant_id: String,
    pub requester_user_id: i64,
    pub background_job_id: i64,
    pub idempotency_key_hash: String,
    pub source_file_id: i64,
    pub source_name_snapshot: String,
    pub source_sha256: String,
    pub duplicate_policy: String,
    pub status: String,
    pub total_rows: i32,
    pub processed_rows: i32,
    pub success_count: i32,
    pub skipped_count: i32,
    pub failure_count: i32,
    pub cancel_requested: bool,
    pub error_report_file_id: Option<i64>,
    pub last_error: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl UserImportJobRecord {
    pub const STATUS_PENDING: &'static str = "pending";
    pub const STATUS_RUNNING: &'static str = "running";
    pub const STATUS_SUCCEEDED: &'static str = "succeeded";
    pub const STATUS_PARTIAL: &'static str = "partial";
    pub const STATUS_FAILED: &'static str = "failed";
    pub const STATUS_CANCELLED: &'static str = "cancelled";

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            Self::STATUS_SUCCEEDED
                | Self::STATUS_PARTIAL
                | Self::STATUS_FAILED
                | Self::STATUS_CANCELLED
        )
    }
}

#[derive(Debug)]
pub struct UserImportRowRecord {
    pub row_number: i32,
    pub username: String,
    pub outcome: String,
    pub code: String,
    pub message: String,
    pub created_at: DateTime<Utc>,
}

impl UserImportRowRecord {
    pub const OUTCOME_SKIPPED: &'static str = "skipped";
    pub const OUTCOME_FAILED: &'static str = "failed";
}

#[derive(Debug)]
pub struct NewImportedUser {
    pub id: i64,
    pub tenant_id: String,
    pub username: String,
    pub password_hash: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub department_id: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct NewUserImportRow {
    pub id: i64,
    pub tenant_id: String,
    pub import_job_id: i64,
    pub row_number: i32,
    pub username: String,
    pub outcome: String,
    pub code: String,
    pub message: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug)]
pub struct UserImportAuthorizationSnapshot {
    pub tenant_epoch: i32,
    pub tenant_available: bool,
    pub requester_enabled: bool,
    pub requester_version: Option<i32>,
}

impl UserImportAuthorizationSnapshot {
    pub fn matches(self, tenant_epoch: i32, requester_version: i32) -> bool {
        self.tenant_available
            && self.requester_enabled
            && self.tenant_epoch == tenant_epoch
            && self.requester_version == Some(requester_version)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserImportSourceState {
    Ready,
    Recoverable,
    Unavailable,
}

#[derive(Debug)]
pub struct UserImportSourceRecord {
    pub bucket: String,
    pub sha256: String,
    pub state: UserImportSourceState,
}

#[derive(Debug)]
pub struct NewUserImportJob {
    pub id: i64,
    pub tenant_id: String,
    pub requester_user_id: i64,
    pub background_job_id: i64,
    pub idempotency_key_hash: String,
    pub source_file_id: i64,
    pub source_name: String,
    pub source_sha256: String,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UserImportReadFilter<'a> {
    pub status: Option<&'a str>,
}

#[async_trait::async_trait]
pub trait UserImportTransaction: BackgroundJobTransaction {
    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<()>;

    async fn find_by_idempotency<'a>(
        &'a self,
        tenant_id: &'a str,
        idempotency_key_hash: &'a str,
    ) -> ryframe_kernel::AppResult<Option<UserImportJobRecord>>;

    async fn requester_username<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<Option<String>>;

    async fn active_count<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<u64>;

    async fn lock_source<'a>(
        &'a self,
        tenant_id: &'a str,
        source_file_id: i64,
    ) -> ryframe_kernel::AppResult<Option<UserImportSourceRecord>>;

    async fn restore_source<'a>(
        &'a self,
        tenant_id: &'a str,
        source_file_id: i64,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn create(
        &self,
        job: NewUserImportJob,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<UserImportJobRecord>;

    async fn mark_source_for_cleanup<'a>(
        &'a self,
        tenant_id: &'a str,
        source_file_id: i64,
        now: DateTime<Utc>,
        cleanup_after: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn lock_configuration(&self, import_id: i64)
    -> ryframe_kernel::AppResult<Option<String>>;

    async fn lock_authorization<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_user_id: i64,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<UserImportAuthorizationSnapshot>;

    async fn existing_usernames<'a>(
        &'a self,
        tenant_id: &'a str,
        usernames: &'a [String],
    ) -> ryframe_kernel::AppResult<Vec<String>>;

    async fn ensure_user_quota<'a>(
        &'a self,
        tenant_id: &'a str,
        additional_users: usize,
    ) -> ryframe_kernel::AppResult<()>;

    async fn insert_users<'a>(
        &'a self,
        tenant_id: &'a str,
        users: Vec<NewImportedUser>,
    ) -> ryframe_kernel::AppResult<()>;

    async fn insert_rows(&self, rows: Vec<NewUserImportRow>) -> ryframe_kernel::AppResult<()>;

    async fn lock(&self, import_id: i64) -> ryframe_kernel::AppResult<Option<UserImportJobRecord>>;

    async fn save(
        &self,
        record: UserImportJobRecord,
    ) -> ryframe_kernel::AppResult<UserImportJobRecord>;

    async fn commit(self: Box<Self>) -> ryframe_kernel::AppResult<()>;

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()>;
}

#[async_trait::async_trait]
pub trait UserImportPersistencePort: Send + Sync {
    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn UserImportTransaction>>;

    async fn list_departments<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<Vec<UserImportDepartmentRecord>>;

    async fn list<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: UserImportReadFilter<'a>,
    ) -> ryframe_kernel::AppResult<PageResult<UserImportJobRecord>>;

    async fn find<'a>(
        &'a self,
        tenant_id: &'a str,
        import_id: i64,
    ) -> ryframe_kernel::AppResult<Option<UserImportJobRecord>>;

    async fn find_global(
        &self,
        import_id: i64,
    ) -> ryframe_kernel::AppResult<Option<UserImportJobRecord>>;

    async fn find_by_background_job(
        &self,
        background_job_id: i64,
    ) -> ryframe_kernel::AppResult<Option<UserImportJobRecord>>;

    async fn rows<'a>(
        &'a self,
        tenant_id: &'a str,
        import_id: i64,
        page: ValidatedPageQuery,
    ) -> ryframe_kernel::AppResult<PageResult<UserImportRowRecord>>;

    async fn all_rows<'a>(
        &'a self,
        tenant_id: &'a str,
        import_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<UserImportRowRecord>>;

    async fn requester_usernames<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<Vec<(i64, String)>>;

    async fn request_cancel<'a>(
        &'a self,
        tenant_id: &'a str,
        import_id: i64,
    ) -> ryframe_kernel::AppResult<bool>;
}
