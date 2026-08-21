use chrono::{DateTime, Utc};
use ryframe_kernel::{PageResult, ValidatedPageQuery};

use crate::{PersistenceFuture, ports::jobs::BackgroundJobTransaction};

#[derive(Clone, Debug)]
pub struct RetentionRunRecord {
    pub id: i64,
    pub background_job_id: i64,
    pub trigger_kind: String,
    pub status: String,
    pub policy_snapshot: serde_json::Value,
    pub eligible_counts: serde_json::Value,
    pub deleted_counts: serde_json::Value,
    pub remaining_counts: serde_json::Value,
    pub requested_by: Option<i64>,
    pub error_summary: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl RetentionRunRecord {
    pub const TRIGGER_SCHEDULED: &'static str = "scheduled";
    pub const TRIGGER_MANUAL: &'static str = "manual";
    pub const STATUS_PENDING: &'static str = "pending";
    pub const STATUS_RUNNING: &'static str = "running";
    pub const STATUS_SUCCEEDED: &'static str = "succeeded";
    pub const STATUS_PARTIAL: &'static str = "partial";
    pub const STATUS_FAILED: &'static str = "failed";
}

pub trait RetentionRunTransaction: Send + Sync {
    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn background_jobs(&self) -> &dyn BackgroundJobTransaction;

    fn find_by_background_job(
        &self,
        background_job_id: i64,
    ) -> PersistenceFuture<'_, Option<RetentionRunRecord>>;

    fn insert_if_missing(
        &self,
        record: RetentionRunRecord,
    ) -> PersistenceFuture<'_, RetentionRunRecord>;

    fn lock_by_background_job(
        &self,
        background_job_id: i64,
    ) -> PersistenceFuture<'_, Option<RetentionRunRecord>>;

    fn begin_run(
        &self,
        record: RetentionRunRecord,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'_, Option<RetentionRunRecord>>;

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

pub trait RetentionRunPersistencePort: Send + Sync {
    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn RetentionRunTransaction>>;

    fn insert_if_missing(
        &self,
        record: RetentionRunRecord,
    ) -> PersistenceFuture<'_, RetentionRunRecord>;

    fn update(&self, record: RetentionRunRecord) -> PersistenceFuture<'_, RetentionRunRecord>;

    fn list(
        &self,
        page: ValidatedPageQuery,
    ) -> PersistenceFuture<'_, PageResult<RetentionRunRecord>>;
}
