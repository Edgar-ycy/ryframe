use std::time::Duration as StdDuration;

use ryframe_kernel::{PageResult, ValidatedPageQuery};
use time::{Duration, OffsetDateTime};

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
    /// 本次领取的单调序号，用于拒绝同一 Worker 标识的旧租约结果。
    pub claim_sequence: i64,
    pub max_attempts: i32,
    pub max_runtime_seconds: Option<i32>,
    pub traceparent: Option<String>,
    pub tracestate: Option<String>,
}

pub struct FailJobCommand<'a> {
    pub job_id: i64,
    pub claim_sequence: i64,
    pub worker_id: &'a str,
    pub retry_at: OffsetDateTime,
    pub error_message: &'a str,
    pub force_dead: bool,
    pub now: OffsetDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobFailureOutcome {
    Retried {
        available_at: OffsetDateTime,
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
    pub scheduled_for: Option<OffsetDateTime>,
    pub max_runtime_seconds: Option<i32>,
    pub job_type: String,
    pub status: String,
    pub priority: i32,
    pub available_at: OffsetDateTime,
    pub attempts: i32,
    pub max_attempts: i32,
    pub lease_owner: Option<String>,
    pub lease_until: Option<OffsetDateTime>,
    pub dedupe_key: Option<String>,
    pub last_error: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub completed_at: Option<OffsetDateTime>,
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
        now: OffsetDateTime,
    ) -> ryframe_kernel::AppResult<bool>;
}

/// 后台任务队列使用的控制库持久化端口。
#[async_trait::async_trait]
pub trait BackgroundJobPersistencePort: Send + Sync {
    async fn database_now(&self) -> ryframe_kernel::AppResult<OffsetDateTime>;

    async fn claim_next<'a>(
        &'a self,
        worker_id: &'a str,
        lease_duration: Duration,
        now: OffsetDateTime,
        tenant_scope: &'a ExecutionTenantScope,
    ) -> ryframe_kernel::AppResult<Option<ClaimedJobRecord>>;

    async fn dead_letter<'a>(
        &'a self,
        job_id: i64,
        claim_sequence: i64,
        worker_id: &'a str,
        error_message: &'a str,
        now: OffsetDateTime,
    ) -> ryframe_kernel::AppResult<JobFailureOutcome>;

    async fn renew_lease<'a>(
        &'a self,
        job_id: i64,
        claim_sequence: i64,
        worker_id: &'a str,
        lease_duration: Duration,
        now: OffsetDateTime,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn complete<'a>(
        &'a self,
        job_id: i64,
        claim_sequence: i64,
        worker_id: &'a str,
        now: OffsetDateTime,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn defer_retryable_conflict<'a>(
        &'a self,
        job_id: i64,
        claim_sequence: i64,
        worker_id: &'a str,
        available_at: OffsetDateTime,
        error_message: &'a str,
        now: OffsetDateTime,
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
        now: OffsetDateTime,
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
        now: OffsetDateTime,
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
        now: OffsetDateTime,
    ) -> ryframe_kernel::AppResult<JobFailureOutcome>;

    async fn tenant_config_job_owner<'a>(
        &'a self,
        tenant_id: &'a str,
        job_id: i64,
        kind: TenantConfigJobKind,
    ) -> ryframe_kernel::AppResult<Option<i64>>;
}
