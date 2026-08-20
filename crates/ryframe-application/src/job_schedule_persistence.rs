use chrono::{DateTime, Utc};
use ryframe_kernel::{PageResult, ValidatedPageQuery};

use crate::{EnqueueJob, EnqueueJobResult, ExecutionTenantScope, PersistenceFuture};

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
    pub tenant_id: String,
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
    pub deleted: bool,
}

#[derive(Debug)]
pub struct JobScheduleExecutionRecord {
    pub id: i64,
    pub tenant_id: String,
    pub schedule_id: i64,
    pub schedule_name: String,
    pub handler_key: String,
    pub fire_key: String,
    pub trigger_kind: String,
    pub scheduled_for: DateTime<Utc>,
    pub outcome: String,
    pub background_job_id: Option<i64>,
    pub background_job_status: Option<String>,
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct NewJobScheduleExecution {
    pub id: i64,
    pub fire_key: String,
    pub trigger_kind: String,
    pub scheduled_for: DateTime<Utc>,
    pub outcome: String,
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

pub trait JobScheduleTransaction: Send + Sync {
    fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()>;

    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn count_enabled<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, u64>;

    fn lock_schedule<'a>(
        &'a self,
        tenant_id: &'a str,
        schedule_id: i64,
    ) -> PersistenceFuture<'a, Option<JobScheduleRecord>>;

    fn lock_next_due<'a>(
        &'a self,
        now: DateTime<Utc>,
        tenant_scope: &'a ExecutionTenantScope,
    ) -> PersistenceFuture<'a, Option<JobScheduleRecord>>;

    fn has_active_job(&self, schedule_id: i64) -> PersistenceFuture<'_, bool>;

    fn find_execution_by_fire_key<'a>(
        &'a self,
        schedule_id: i64,
        fire_key: &'a str,
    ) -> PersistenceFuture<'a, Option<JobScheduleExecutionRecord>>;

    fn insert_schedule(
        &self,
        schedule: JobScheduleRecord,
    ) -> PersistenceFuture<'_, JobScheduleRecord>;

    fn save_schedule(
        &self,
        schedule: JobScheduleRecord,
    ) -> PersistenceFuture<'_, JobScheduleRecord>;

    fn insert_execution<'a>(
        &'a self,
        schedule: &'a JobScheduleRecord,
        execution: NewJobScheduleExecution,
    ) -> PersistenceFuture<'a, JobScheduleExecutionRecord>;

    fn attach_background_job(
        &self,
        execution: JobScheduleExecutionRecord,
        background_job_id: i64,
    ) -> PersistenceFuture<'_, JobScheduleExecutionRecord>;

    fn enqueue(&self, command: EnqueueJob) -> PersistenceFuture<'_, EnqueueJobResult>;

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

pub trait JobSchedulePersistencePort: JobScheduleReadPort {
    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn JobScheduleTransaction>>;
}
