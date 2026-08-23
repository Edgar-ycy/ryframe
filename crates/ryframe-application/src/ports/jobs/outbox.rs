use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

use crate::{EnqueueJob, ports::system::OperLogRecord};

use super::ExecutionTenantScope;

/// 已由 Worker 持有租约的 Outbox 事件。
#[derive(Debug)]
pub struct ClaimedOutboxEvent {
    pub id: i64,
    pub tenant_id: Option<String>,
    pub event_type: String,
    pub payload: Value,
    pub attempts: i32,
    pub max_attempts: i32,
    pub dedupe_key: Option<String>,
    pub traceparent: Option<String>,
    pub tracestate: Option<String>,
}

/// Outbox 投递失败后的状态转换结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutboxFailureOutcome {
    Retried { available_at: DateTime<Utc> },
    Dead,
    LeaseLost,
}

/// Outbox Worker 所需的控制库持久化端口。
#[async_trait::async_trait]
pub trait OutboxPersistencePort: Send + Sync {
    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn claim_next<'a>(
        &'a self,
        worker_id: &'a str,
        lease_duration: Duration,
        now: DateTime<Utc>,
        tenant_scope: &'a ExecutionTenantScope,
    ) -> ryframe_kernel::AppResult<Option<ClaimedOutboxEvent>>;

    async fn publish_background_job<'a>(
        &'a self,
        event_id: i64,
        worker_id: &'a str,
        command: EnqueueJob,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn publish_audit<'a>(
        &'a self,
        event_id: i64,
        worker_id: &'a str,
        tenant_id: &'a str,
        record: OperLogRecord,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn mark_published<'a>(
        &'a self,
        event_id: i64,
        worker_id: &'a str,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn fail<'a>(
        &'a self,
        event_id: i64,
        worker_id: &'a str,
        retry_at: DateTime<Utc>,
        error_message: &'a str,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<OutboxFailureOutcome>;

    async fn recover_expired_leases<'a>(
        &'a self,
        now: DateTime<Utc>,
        tenant_scope: &'a ExecutionTenantScope,
    ) -> ryframe_kernel::AppResult<()>;
}
