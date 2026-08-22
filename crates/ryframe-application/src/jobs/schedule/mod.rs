use std::{str::FromStr, sync::Arc, time::Duration as StdDuration};

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use ryframe_kernel::{ActorContext, AppError, AppResult, PageResult, ValidatedPageQuery};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::{sync::watch, task::JoinHandle, time};

use super::{
    JobQueue, ScheduleMetricsObserver, ScheduledJobContext, ScheduledJobTarget,
    ScheduledJobTargetDescriptor, ScheduledJobTargetRegistry, ScheduledJobTargetScope,
};
use crate::ports::jobs::{
    ExecutionTenantScope, JobScheduleExecutionReadFilter, JobScheduleExecutionRecord,
    JobSchedulePersistencePort, JobScheduleReadFilter, JobScheduleRecord, JobScheduleTransaction,
    NewJobScheduleExecution,
};

const SYSTEM_TENANT_ID: &str = "system";
const MAX_NAME_BYTES: usize = 100;
const MAX_CRON_BYTES: usize = 191;
const MAX_TIMEZONE_BYTES: usize = 64;
const MISFIRE_SKIP: &str = "skip";
const MISFIRE_FIRE_ONCE: &str = "fire_once";
const CONCURRENCY_FORBID: &str = "forbid";
const CONCURRENCY_ALLOW: &str = "allow";
const TRIGGER_SCHEDULED: &str = "scheduled";
const TRIGGER_MISFIRE: &str = "misfire";
const TRIGGER_MANUAL: &str = "manual";
const OUTCOME_ENQUEUED: &str = "enqueued";
const OUTCOME_SKIPPED_MISFIRE: &str = "skipped_misfire";
const OUTCOME_SKIPPED_CONCURRENCY: &str = "skipped_concurrency";
const OUTCOME_TARGET_UNAVAILABLE: &str = "target_unavailable";
const OUTCOME_INVALID_CONFIGURATION: &str = "invalid_configuration";

#[derive(Clone, Debug)]
pub struct JobScheduleListParams {
    pub page: ValidatedPageQuery,
    pub name: Option<String>,
    pub handler_key: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct JobScheduleExecutionListParams {
    pub page: ValidatedPageQuery,
    pub trigger_kind: Option<String>,
    pub outcome: Option<String>,
    pub background_job_status: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CreateJobSchedule {
    pub name: String,
    pub handler_key: String,
    pub cron_expression: String,
    pub timezone: String,
    pub enabled: bool,
    pub misfire_policy: String,
    pub concurrency_policy: String,
    pub max_runtime_seconds: i32,
}

#[derive(Clone, Debug)]
pub struct UpdateJobSchedule {
    pub version: i64,
    pub name: String,
    pub handler_key: String,
    pub cron_expression: String,
    pub timezone: String,
    pub enabled: bool,
    pub misfire_policy: String,
    pub concurrency_policy: String,
    pub max_runtime_seconds: i32,
}

#[derive(Clone, Debug, Serialize)]
pub struct JobScheduleVo {
    pub id: String,
    pub name: String,
    pub handler_key: String,
    pub cron_expression: String,
    pub timezone: String,
    pub enabled: bool,
    pub misfire_policy: String,
    pub concurrency_policy: String,
    pub max_runtime_seconds: i32,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<JobScheduleRecord> for JobScheduleVo {
    fn from(schedule: JobScheduleRecord) -> Self {
        Self {
            id: schedule.id.to_string(),
            name: schedule.name,
            handler_key: schedule.handler_key,
            cron_expression: schedule.cron_expression,
            timezone: schedule.timezone,
            enabled: schedule.enabled,
            misfire_policy: schedule.misfire_policy,
            concurrency_policy: schedule.concurrency_policy,
            max_runtime_seconds: schedule.max_runtime_seconds,
            next_run_at: schedule.next_run_at,
            last_run_at: schedule.last_run_at,
            version: schedule.version,
            created_at: schedule.created_at,
            updated_at: schedule.updated_at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct JobScheduleExecutionVo {
    pub id: String,
    pub schedule_id: String,
    pub schedule_name: String,
    pub handler_key: String,
    pub trigger_kind: String,
    pub scheduled_for: DateTime<Utc>,
    pub outcome: String,
    pub background_job_id: Option<String>,
    pub background_job_status: Option<String>,
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<JobScheduleExecutionRecord> for JobScheduleExecutionVo {
    fn from(execution: JobScheduleExecutionRecord) -> Self {
        Self {
            id: execution.id.to_string(),
            schedule_id: execution.schedule_id.to_string(),
            schedule_name: execution.schedule_name,
            handler_key: execution.handler_key,
            trigger_kind: execution.trigger_kind,
            scheduled_for: execution.scheduled_for,
            outcome: execution.outcome,
            background_job_id: execution.background_job_id.map(|id| id.to_string()),
            background_job_status: execution.background_job_status,
            detail: execution.detail,
            created_at: execution.created_at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct JobScheduleOccurrence {
    pub utc: DateTime<Utc>,
    pub schedule_time: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct JobSchedulePreview {
    pub calculated_at: DateTime<Utc>,
    pub timezone: String,
    pub occurrences: Vec<JobScheduleOccurrence>,
}

/// 数据库驱动的租户调度服务。
#[derive(Clone)]
pub struct JobScheduleService {
    persistence: Arc<dyn JobSchedulePersistencePort>,
    queue: Arc<JobQueue>,
    execution_tenant_scope: ExecutionTenantScope,
    targets: ScheduledJobTargetRegistry,
    metrics: Option<Arc<dyn ScheduleMetricsObserver>>,
    poll_interval: StdDuration,
    batch_size: usize,
    max_enabled_per_tenant: usize,
}

mod execution;
mod expression;
mod management;
mod persistence;

use expression::*;
use persistence::*;

/// 校验数据库中已启用计划的完整配置，不向调用方暴露解析器实现。
pub fn validate_persisted_schedule_configuration(
    next_run_at: Option<DateTime<Utc>>,
    misfire_policy: &str,
    concurrency_policy: &str,
    max_runtime_seconds: i32,
    cron_expression: &str,
    timezone: &str,
    now: DateTime<Utc>,
) -> Result<(), String> {
    validate_persisted_schedule(
        next_run_at,
        misfire_policy,
        concurrency_policy,
        max_runtime_seconds,
        cron_expression,
        timezone,
        now,
    )
    .map(|_| ())
}
