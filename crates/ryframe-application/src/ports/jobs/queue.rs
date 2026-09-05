use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use ryframe_kernel::{PageResult, ValidatedPageQuery};

use crate::{EnqueueJob, EnqueueJobResult};

use super::ExecutionTenantScope;

#[derive(Debug)]
pub struct ClaimedJobRecord {
    pub id: i64,
    pub tenant_id: Option<String>,
    pub job_type: String,
    pub payload: serde_json::Value,
    pub lease_owner: Option<String>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub max_runtime_seconds: Option<i32>,
    pub traceparent: Option<String>,
    pub tracestate: Option<String>,
}

pub struct FailJobCommand<'a> {
    pub job_id: i64,
    pub worker_id: &'a str,
    pub retry_at: DateTime<Utc>,
    pub error_message: &'a str,
    pub force_dead: bool,
    pub now: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobFailureOutcome {
    Retried {
        available_at: DateTime<Utc>,
    },
    Dead,
    /// 关联业务已经权威终结，后台任务已按成功完成收口。
    Completed,
    LeaseLost,
}

#[derive(Debug)]
pub struct BackgroundJobRecord {
    pub id: i64,
    pub tenant_id: Option<String>,
    pub schedule_id: Option<i64>,
    pub scheduled_for: Option<DateTime<Utc>>,
    pub max_runtime_seconds: Option<i32>,
    pub job_type: String,
    pub status: String,
    pub priority: i32,
    pub available_at: DateTime<Utc>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub lease_owner: Option<String>,
    pub lease_until: Option<DateTime<Utc>>,
    pub dedupe_key: Option<String>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl BackgroundJobRecord {
    pub const STATUS_PENDING: &'static str = "pending";
    pub const STATUS_RUNNING: &'static str = "running";
    pub const STATUS_SUCCEEDED: &'static str = "succeeded";
    pub const STATUS_DEAD: &'static str = "dead";
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BackgroundJobStatsRecord {
    pub total: u64,
    pub pending: u64,
    pub running: u64,
    pub succeeded: u64,
    pub dead: u64,
    pub ready: u64,
}

#[derive(Debug)]
pub struct BackgroundJobTypeStats {
    pub job_type: String,
    pub pending: u64,
    pub running: u64,
    pub dead: u64,
    pub ready: u64,
    pub oldest_ready_age: Option<StdDuration>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RecoveredJobLeases {
    pub requeued: u64,
    pub dead: u64,
    pub completed: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BackgroundJobReadFilter<'a> {
    pub tenant_id: Option<&'a str>,
    pub include_platform: bool,
    pub schedule_id: Option<i64>,
    pub job_type: Option<&'a str>,
    pub status: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TenantConfigJobKind {
    Export,
    Preview,
    Apply,
    Rollback,
}

/// 已由调用方持有的控制库事务提供的任务原子写能力。
#[async_trait::async_trait]
pub trait BackgroundJobTransaction: Send + Sync {
    async fn enqueue(&self, command: EnqueueJob) -> ryframe_kernel::AppResult<EnqueueJobResult>;

    async fn reactivate_linked<'a>(
        &'a self,
        job_id: i64,
        expected_job_type: &'a str,
        payload_key: &'a str,
        expected_resource_id: i64,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;
}

/// 后台任务队列使用的控制库持久化端口。
#[async_trait::async_trait]
pub trait BackgroundJobPersistencePort: Send + Sync {
    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn claim_next<'a>(
        &'a self,
        worker_id: &'a str,
        lease_duration: Duration,
        now: DateTime<Utc>,
        tenant_scope: &'a ExecutionTenantScope,
    ) -> ryframe_kernel::AppResult<Option<ClaimedJobRecord>>;

    async fn dead_letter<'a>(
        &'a self,
        job_id: i64,
        worker_id: &'a str,
        error_message: &'a str,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<JobFailureOutcome>;

    async fn renew_lease<'a>(
        &'a self,
        job_id: i64,
        worker_id: &'a str,
        lease_duration: Duration,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn complete<'a>(
        &'a self,
        job_id: i64,
        worker_id: &'a str,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn defer_retryable_conflict<'a>(
        &'a self,
        job_id: i64,
        worker_id: &'a str,
        available_at: DateTime<Utc>,
        error_message: &'a str,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<JobFailureOutcome>;

    async fn fail<'a>(
        &'a self,
        command: FailJobCommand<'a>,
    ) -> ryframe_kernel::AppResult<JobFailureOutcome>;

    async fn stats_for_types<'a>(
        &'a self,
        job_types: &'a [String],
        tenant_scope: &'a ExecutionTenantScope,
    ) -> ryframe_kernel::AppResult<Vec<BackgroundJobTypeStats>>;

    async fn recover_expired_leases<'a>(
        &'a self,
        now: DateTime<Utc>,
        tenant_scope: &'a ExecutionTenantScope,
    ) -> ryframe_kernel::AppResult<RecoveredJobLeases>;

    async fn enqueue(&self, command: EnqueueJob) -> ryframe_kernel::AppResult<EnqueueJobResult>;

    async fn list<'a>(
        &'a self,
        filter: BackgroundJobReadFilter<'a>,
        page: ValidatedPageQuery,
    ) -> ryframe_kernel::AppResult<PageResult<BackgroundJobRecord>>;

    async fn stats<'a>(
        &'a self,
        filter: BackgroundJobReadFilter<'a>,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<BackgroundJobStatsRecord>;

    async fn find_for_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
        include_platform: bool,
        job_id: i64,
    ) -> ryframe_kernel::AppResult<Option<BackgroundJobRecord>>;

    async fn retry_dead<'a>(
        &'a self,
        tenant_id: &'a str,
        include_platform: bool,
        job_id: i64,
        retried_by: i64,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn tenant_config_job_owner<'a>(
        &'a self,
        tenant_id: &'a str,
        job_id: i64,
        kind: TenantConfigJobKind,
    ) -> ryframe_kernel::AppResult<Option<i64>>;
}
