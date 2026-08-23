use chrono::{DateTime, Utc};
use ryframe_kernel::{PageResult, ValidatedPageQuery};

use crate::{EnqueueJob, EnqueueJobResult};

use super::ExecutionTenantScope;

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

#[async_trait::async_trait]
pub trait JobScheduleReadPort: Send + Sync {
    async fn page<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: JobScheduleReadFilter<'a>,
        page: ValidatedPageQuery,
    ) -> ryframe_kernel::AppResult<PageResult<JobScheduleRecord>>;

    async fn find<'a>(
        &'a self,
        tenant_id: &'a str,
        schedule_id: i64,
    ) -> ryframe_kernel::AppResult<Option<JobScheduleRecord>>;

    async fn execution_page<'a>(
        &'a self,
        tenant_id: &'a str,
        schedule_id: i64,
        filter: JobScheduleExecutionReadFilter<'a>,
        page: ValidatedPageQuery,
    ) -> ryframe_kernel::AppResult<PageResult<JobScheduleExecutionRecord>>;
}

#[async_trait::async_trait]
pub trait JobScheduleTransaction: Send + Sync {
    async fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<()>;

    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn count_enabled<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<u64>;

    async fn lock_schedule<'a>(
        &'a self,
        tenant_id: &'a str,
        schedule_id: i64,
    ) -> ryframe_kernel::AppResult<Option<JobScheduleRecord>>;

    async fn lock_next_due<'a>(
        &'a self,
        now: DateTime<Utc>,
        tenant_scope: &'a ExecutionTenantScope,
    ) -> ryframe_kernel::AppResult<Option<JobScheduleRecord>>;

    async fn has_active_job(&self, schedule_id: i64) -> ryframe_kernel::AppResult<bool>;

    async fn find_execution_by_fire_key<'a>(
        &'a self,
        schedule_id: i64,
        fire_key: &'a str,
    ) -> ryframe_kernel::AppResult<Option<JobScheduleExecutionRecord>>;

    async fn insert_schedule(
        &self,
        schedule: JobScheduleRecord,
    ) -> ryframe_kernel::AppResult<JobScheduleRecord>;

    async fn save_schedule(
        &self,
        schedule: JobScheduleRecord,
    ) -> ryframe_kernel::AppResult<JobScheduleRecord>;

    async fn insert_execution<'a>(
        &'a self,
        schedule: &'a JobScheduleRecord,
        execution: NewJobScheduleExecution,
    ) -> ryframe_kernel::AppResult<JobScheduleExecutionRecord>;

    async fn attach_background_job(
        &self,
        execution: JobScheduleExecutionRecord,
        background_job_id: i64,
    ) -> ryframe_kernel::AppResult<JobScheduleExecutionRecord>;

    async fn enqueue(&self, command: EnqueueJob) -> ryframe_kernel::AppResult<EnqueueJobResult>;

    async fn commit(self: Box<Self>) -> ryframe_kernel::AppResult<()>;

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()>;
}

#[async_trait::async_trait]
pub trait JobSchedulePersistencePort: JobScheduleReadPort {
    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn JobScheduleTransaction>>;
}
