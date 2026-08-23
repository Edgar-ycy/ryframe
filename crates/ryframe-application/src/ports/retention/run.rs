use chrono::{DateTime, Utc};
use ryframe_kernel::{PageResult, ValidatedPageQuery};

use crate::ports::jobs::BackgroundJobTransaction;

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

#[async_trait::async_trait]
pub trait RetentionRunTransaction: crate::PersistenceTransaction + Send + Sync {
    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    fn background_jobs(&self) -> &dyn BackgroundJobTransaction;

    async fn find_by_background_job(
        &self,
        background_job_id: i64,
    ) -> ryframe_kernel::AppResult<Option<RetentionRunRecord>>;

    async fn insert_if_missing(
        &self,
        record: RetentionRunRecord,
    ) -> ryframe_kernel::AppResult<RetentionRunRecord>;

    async fn lock_by_background_job(
        &self,
        background_job_id: i64,
    ) -> ryframe_kernel::AppResult<Option<RetentionRunRecord>>;

    async fn begin_run(
        &self,
        record: RetentionRunRecord,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<Option<RetentionRunRecord>>;
}

#[async_trait::async_trait]
pub trait RetentionRunPersistencePort: Send + Sync {
    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn RetentionRunTransaction>>;

    async fn insert_if_missing(
        &self,
        record: RetentionRunRecord,
    ) -> ryframe_kernel::AppResult<RetentionRunRecord>;

    async fn update(
        &self,
        record: RetentionRunRecord,
    ) -> ryframe_kernel::AppResult<RetentionRunRecord>;

    async fn list(
        &self,
        page: ValidatedPageQuery,
    ) -> ryframe_kernel::AppResult<PageResult<RetentionRunRecord>>;
}
