use chrono::{DateTime, Utc};
use ryframe_kernel::ValidatedPageQuery;
use serde::Serialize;

use super::public_job_error;
use crate::ports::jobs::{BackgroundJobRecord, BackgroundJobStatsRecord};

#[derive(Clone, Debug)]
pub struct EnqueueJob {
    pub tenant_id: Option<String>,
    pub schedule_id: Option<i64>,
    pub scheduled_for: Option<DateTime<Utc>>,
    pub max_runtime_seconds: Option<i32>,
    pub job_type: String,
    pub payload: serde_json::Value,
    pub priority: i32,
    pub available_at: DateTime<Utc>,
    pub max_attempts: i32,
    pub dedupe_key: Option<String>,
    pub traceparent: Option<String>,
    pub tracestate: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EnqueueJobResult {
    pub job_id: i64,
    pub inserted: bool,
}

/// 后台任务分页列表的业务查询参数。
#[derive(Clone, Debug)]
pub struct BackgroundJobListParams {
    pub page: ValidatedPageQuery,
    pub schedule_id: Option<String>,
    pub job_type: Option<String>,
    pub status: Option<String>,
}

/// 面向管理端的后台任务安全视图。
/// 任务载荷可能包含业务敏感字段，因此监控列表不会返回 `payload`。
#[derive(Clone, Debug, Serialize)]
pub struct BackgroundJobVo {
    pub id: String,
    pub schedule_id: Option<String>,
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

impl From<BackgroundJobRecord> for BackgroundJobVo {
    fn from(job: BackgroundJobRecord) -> Self {
        let last_error = public_job_error(&job.job_type, job.last_error);
        Self {
            id: job.id.to_string(),
            schedule_id: job.schedule_id.map(|id| id.to_string()),
            scheduled_for: job.scheduled_for,
            max_runtime_seconds: job.max_runtime_seconds,
            job_type: job.job_type,
            status: job.status,
            priority: job.priority,
            available_at: job.available_at,
            attempts: job.attempts,
            max_attempts: job.max_attempts,
            lease_owner: job.lease_owner,
            lease_until: job.lease_until,
            dedupe_key: job.dedupe_key,
            last_error,
            created_at: job.created_at,
            updated_at: job.updated_at,
            completed_at: job.completed_at,
        }
    }
}

/// 当前租户的后台任务队列统计。
#[derive(Clone, Copy, Debug, Serialize)]
pub struct BackgroundJobQueueStats {
    pub total: u64,
    pub pending: u64,
    pub running: u64,
    pub succeeded: u64,
    pub dead: u64,
    pub ready: u64,
}

impl From<BackgroundJobStatsRecord> for BackgroundJobQueueStats {
    fn from(stats: BackgroundJobStatsRecord) -> Self {
        Self {
            total: stats.total,
            pending: stats.pending,
            running: stats.running,
            succeeded: stats.succeeded,
            dead: stats.dead,
            ready: stats.ready,
        }
    }
}
