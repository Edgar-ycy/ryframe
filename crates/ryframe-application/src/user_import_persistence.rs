use chrono::{DateTime, Utc};
use ryframe_kernel::{PageResult, ValidatedPageQuery};

use crate::PersistenceFuture;

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

#[derive(Clone, Copy, Debug, Default)]
pub struct UserImportReadFilter<'a> {
    pub status: Option<&'a str>,
}

pub trait UserImportTransaction: Send + Sync {
    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

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
}
