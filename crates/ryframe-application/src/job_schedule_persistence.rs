use chrono::{DateTime, Utc};
use ryframe_kernel::{PageResult, ValidatedPageQuery};

use crate::PersistenceFuture;

#[derive(Clone, Copy, Debug, Default)]
pub struct JobScheduleReadFilter<'a> {
    pub name: Option<&'a str>,
    pub handler_key: Option<&'a str>,
    pub enabled: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct JobScheduleExecutionReadFilter<'a> {
    pub trigger_kind: Option<&'a str>,
    pub outcome: Option<&'a str>,
    pub background_job_status: Option<&'a str>,
}

#[derive(Debug)]
pub struct JobScheduleRecord {
    pub id: i64,
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

#[derive(Debug)]
pub struct JobScheduleExecutionRecord {
    pub id: i64,
    pub schedule_id: i64,
    pub schedule_name: String,
    pub handler_key: String,
    pub trigger_kind: String,
    pub scheduled_for: DateTime<Utc>,
    pub outcome: String,
    pub background_job_id: Option<i64>,
    pub background_job_status: Option<String>,
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub trait JobScheduleReadPort: Send + Sync {
    fn page<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: JobScheduleReadFilter<'a>,
        page: ValidatedPageQuery,
    ) -> PersistenceFuture<'a, PageResult<JobScheduleRecord>>;

    fn find<'a>(
        &'a self,
        tenant_id: &'a str,
        schedule_id: i64,
    ) -> PersistenceFuture<'a, Option<JobScheduleRecord>>;

    fn execution_page<'a>(
        &'a self,
        tenant_id: &'a str,
        schedule_id: i64,
        filter: JobScheduleExecutionReadFilter<'a>,
        page: ValidatedPageQuery,
    ) -> PersistenceFuture<'a, PageResult<JobScheduleExecutionRecord>>;
}
