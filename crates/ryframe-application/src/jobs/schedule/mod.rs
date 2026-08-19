use std::{str::FromStr, sync::Arc, time::Duration as StdDuration};

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use ryframe_adapters::snowflake;
use ryframe_config::JobConfig;
use ryframe_db::{
    ControlDatabaseCluster, ExecutionTenantScope, JobScheduleExecutionFilter, JobScheduleFilter,
    JobScheduleRepository, job_schedule, job_schedule_execution,
};
use ryframe_kernel::{ActorContext, AppError, AppResult, PageResult, ValidatedPageQuery};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseTransaction, TransactionTrait,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::{sync::watch, task::JoinHandle, time};

use super::{
    JobQueue, ScheduleMetricsObserver, ScheduledJobContext, ScheduledJobTarget,
    ScheduledJobTargetDescriptor, ScheduledJobTargetRegistry, ScheduledJobTargetScope,
};

const SYSTEM_TENANT_ID: &str = "system";
const MAX_NAME_BYTES: usize = 100;
const MAX_CRON_BYTES: usize = 191;
const MAX_TIMEZONE_BYTES: usize = 64;

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

impl From<job_schedule::Model> for JobScheduleVo {
    fn from(schedule: job_schedule::Model) -> Self {
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
    database: ControlDatabaseCluster,
    repository: Arc<JobScheduleRepository>,
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
