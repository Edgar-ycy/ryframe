use chrono::{DateTime, Utc};
use ryframe_kernel::{PageResult, ValidatedPageQuery};

use crate::{BackgroundJobTransaction, PersistenceFuture};

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

pub trait UserImportTransaction: BackgroundJobTransaction {
    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()>;

    fn find_by_idempotency<'a>(
        &'a self,
        tenant_id: &'a str,
        idempotency_key_hash: &'a str,
    ) -> PersistenceFuture<'a, Option<UserImportJobRecord>>;

    fn requester_username<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Option<String>>;

    fn active_count<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, u64>;

    fn lock_source<'a>(
        &'a self,
        tenant_id: &'a str,
        source_file_id: i64,
    ) -> PersistenceFuture<'a, Option<UserImportSourceRecord>>;

    fn restore_source<'a>(
        &'a self,
        tenant_id: &'a str,
        source_file_id: i64,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn create(
        &self,
        job: NewUserImportJob,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'_, UserImportJobRecord>;

    fn mark_source_for_cleanup<'a>(
        &'a self,
        tenant_id: &'a str,
        source_file_id: i64,
        now: DateTime<Utc>,
        cleanup_after: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn lock_configuration(&self, import_id: i64) -> PersistenceFuture<'_, Option<String>>;

    fn lock_authorization<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_user_id: i64,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, UserImportAuthorizationSnapshot>;

    fn existing_usernames<'a>(
        &'a self,
        tenant_id: &'a str,
        usernames: &'a [String],
    ) -> PersistenceFuture<'a, Vec<String>>;

    fn ensure_user_quota<'a>(
        &'a self,
        tenant_id: &'a str,
        additional_users: usize,
    ) -> PersistenceFuture<'a, ()>;

    fn insert_users<'a>(
        &'a self,
        tenant_id: &'a str,
        users: Vec<NewImportedUser>,
    ) -> PersistenceFuture<'a, ()>;

    fn insert_rows(&self, rows: Vec<NewUserImportRow>) -> PersistenceFuture<'_, ()>;

    fn lock(&self, import_id: i64) -> PersistenceFuture<'_, Option<UserImportJobRecord>>;

    fn save(&self, record: UserImportJobRecord) -> PersistenceFuture<'_, UserImportJobRecord>;

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

pub trait UserImportPersistencePort: Send + Sync {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn UserImportTransaction>>;

    fn list_departments<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Vec<UserImportDepartmentRecord>>;

    fn list<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: UserImportReadFilter<'a>,
    ) -> PersistenceFuture<'a, PageResult<UserImportJobRecord>>;

    fn find<'a>(
        &'a self,
        tenant_id: &'a str,
        import_id: i64,
    ) -> PersistenceFuture<'a, Option<UserImportJobRecord>>;

    fn find_global(&self, import_id: i64) -> PersistenceFuture<'_, Option<UserImportJobRecord>>;

    fn find_by_background_job(
        &self,
        background_job_id: i64,
    ) -> PersistenceFuture<'_, Option<UserImportJobRecord>>;

    fn rows<'a>(
        &'a self,
        tenant_id: &'a str,
        import_id: i64,
        page: ValidatedPageQuery,
    ) -> PersistenceFuture<'a, PageResult<UserImportRowRecord>>;

    fn all_rows<'a>(
        &'a self,
        tenant_id: &'a str,
        import_id: i64,
    ) -> PersistenceFuture<'a, Vec<UserImportRowRecord>>;

    fn requester_usernames<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> PersistenceFuture<'a, Vec<(i64, String)>>;

    fn request_cancel<'a>(
        &'a self,
        tenant_id: &'a str,
        import_id: i64,
    ) -> PersistenceFuture<'a, bool>;
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn terminal_statuses_are_application_owned() {
        let at = Utc.with_ymd_and_hms(2026, 8, 21, 8, 0, 0).unwrap();
        let mut record = UserImportJobRecord {
            id: 1,
            tenant_id: "tenant-a".into(),
            requester_user_id: 2,
            background_job_id: 3,
            idempotency_key_hash: "a".repeat(64),
            source_file_id: 4,
            source_name_snapshot: "users.xlsx".into(),
            source_sha256: "b".repeat(64),
            duplicate_policy: "skip_existing".into(),
            status: UserImportJobRecord::STATUS_RUNNING.into(),
            total_rows: 1,
            processed_rows: 0,
            success_count: 0,
            skipped_count: 0,
            failure_count: 0,
            cancel_requested: false,
            error_report_file_id: None,
            last_error: None,
            started_at: Some(at),
            completed_at: None,
            created_at: at,
            updated_at: at,
        };

        assert!(!record.is_terminal());
        record.status = UserImportJobRecord::STATUS_PARTIAL.into();
        assert!(record.is_terminal());
    }

    #[test]
    fn authorization_snapshot_requires_exact_versions() {
        let current = UserImportAuthorizationSnapshot {
            tenant_epoch: 2,
            tenant_available: true,
            requester_enabled: true,
            requester_version: Some(3),
        };

        assert!(current.matches(2, 3));
        assert!(!current.matches(1, 3));
        assert!(!current.matches(2, 4));
        assert!(
            !UserImportAuthorizationSnapshot {
                requester_enabled: false,
                ..current
            }
            .matches(2, 3)
        );
    }
}
