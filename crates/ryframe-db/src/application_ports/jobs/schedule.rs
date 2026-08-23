use std::sync::Arc;

use crate::{
    BackgroundJobRepository, ControlDatabaseCluster, JobScheduleExecutionFilter, JobScheduleFilter,
    JobScheduleRepository,
    entities::{background_job, job_schedule, job_schedule_execution},
};
use ryframe_kernel::{AppError, PageResult, ValidatedPageQuery};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseTransaction, EntityTrait,
    TransactionTrait,
};

use ryframe_application::{
    EnqueueJob, EnqueueJobResult,
    ports::jobs::{
        ExecutionTenantScope, JobScheduleExecutionReadFilter, JobScheduleExecutionRecord,
        JobSchedulePersistencePort, JobScheduleReadFilter, JobScheduleReadPort, JobScheduleRecord,
        JobScheduleTransaction, NewJobScheduleExecution,
    },
};

use super::{queue::database_enqueue, tenant_scope::database_scope};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn JobSchedulePersistencePort> {
    Arc::new(DatabaseJobSchedulePersistence {
        database,
        repository: JobScheduleRepository,
    })
}

struct DatabaseJobSchedulePersistence {
    database: ControlDatabaseCluster,
    repository: JobScheduleRepository,
}

#[async_trait::async_trait]
impl JobScheduleReadPort for DatabaseJobSchedulePersistence {
    async fn page<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: JobScheduleReadFilter<'a>,
        page: ValidatedPageQuery,
    ) -> ryframe_kernel::AppResult<PageResult<JobScheduleRecord>> {
        let result = self
            .repository
            .list(
                self.database.write(),
                tenant_id,
                JobScheduleFilter {
                    name: filter.name,
                    handler_key: filter.handler_key,
                    enabled: filter.enabled,
                },
                &page,
            )
            .await?;
        Ok(PageResult::new(
            result.records.into_iter().map(to_schedule).collect(),
            result.total,
            &page,
        ))
    }

    async fn find<'a>(
        &'a self,
        tenant_id: &'a str,
        schedule_id: i64,
    ) -> ryframe_kernel::AppResult<Option<JobScheduleRecord>> {
        Ok(self
            .repository
            .find_for_tenant(self.database.write(), tenant_id, schedule_id)
            .await?
            .map(to_schedule))
    }

    async fn execution_page<'a>(
        &'a self,
        tenant_id: &'a str,
        schedule_id: i64,
        filter: JobScheduleExecutionReadFilter<'a>,
        page: ValidatedPageQuery,
    ) -> ryframe_kernel::AppResult<PageResult<JobScheduleExecutionRecord>> {
        let result = self
            .repository
            .list_executions(
                self.database.write(),
                tenant_id,
                schedule_id,
                JobScheduleExecutionFilter {
                    trigger_kind: filter.trigger_kind,
                    outcome: filter.outcome,
                    background_job_status: filter.background_job_status,
                },
                &page,
            )
            .await?;
        let job_ids = result
            .records
            .iter()
            .filter_map(|execution| execution.background_job_id)
            .collect::<Vec<_>>();
        let statuses = self
            .repository
            .background_job_statuses(self.database.write(), &job_ids)
            .await?;
        let records = result
            .records
            .into_iter()
            .map(|execution| {
                let status = execution
                    .background_job_id
                    .and_then(|job_id| statuses.get(&job_id).cloned());
                to_execution(execution, status)
            })
            .collect();
        Ok(PageResult::new(records, result.total, &page))
    }
}

#[async_trait::async_trait]
impl JobSchedulePersistencePort for DatabaseJobSchedulePersistence {
    async fn database_now(&self) -> ryframe_kernel::AppResult<chrono::DateTime<chrono::Utc>> {
        crate::repositories::database_utc_now(self.database.write()).await
    }

    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn JobScheduleTransaction>> {
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        Ok(Box::new(DatabaseJobScheduleTransaction {
            transaction,
            schedule_repository: JobScheduleRepository,
            job_repository: BackgroundJobRepository,
        }) as Box<dyn JobScheduleTransaction>)
    }
}

struct DatabaseJobScheduleTransaction {
    transaction: DatabaseTransaction,
    schedule_repository: JobScheduleRepository,
    job_repository: BackgroundJobRepository,
}

#[async_trait::async_trait]
impl JobScheduleTransaction for DatabaseJobScheduleTransaction {
    async fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<()> {
        let row = self
            .transaction
            .query_one_raw(sea_orm::Statement::from_sql_and_values(
                sea_orm::DbBackend::MySql,
                "SELECT tenant_id FROM sys_tenant WHERE tenant_id = ? FOR UPDATE",
                [tenant_id.into()],
            ))
            .await
            .map_err(database_error)?;
        if row.is_none() {
            return Err(AppError::NotFound("当前租户不存在".into()));
        }
        Ok(())
    }

    async fn database_now(&self) -> ryframe_kernel::AppResult<chrono::DateTime<chrono::Utc>> {
        crate::repositories::database_utc_now(&self.transaction).await
    }

    async fn count_enabled<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<u64> {
        self.schedule_repository
            .count_enabled(&self.transaction, tenant_id)
            .await
    }

    async fn lock_schedule<'a>(
        &'a self,
        tenant_id: &'a str,
        schedule_id: i64,
    ) -> ryframe_kernel::AppResult<Option<JobScheduleRecord>> {
        Ok(self
            .schedule_repository
            .lock_for_tenant(&self.transaction, tenant_id, schedule_id)
            .await?
            .map(to_schedule))
    }

    async fn lock_next_due<'a>(
        &'a self,
        now: chrono::DateTime<chrono::Utc>,
        tenant_scope: &'a ExecutionTenantScope,
    ) -> ryframe_kernel::AppResult<Option<JobScheduleRecord>> {
        let tenant_scope = database_scope(tenant_scope);
        Ok(self
            .schedule_repository
            .lock_next_due(&self.transaction, now, &tenant_scope)
            .await?
            .map(to_schedule))
    }

    async fn has_active_job(&self, schedule_id: i64) -> ryframe_kernel::AppResult<bool> {
        self.schedule_repository
            .has_active_job(&self.transaction, schedule_id)
            .await
    }

    async fn find_execution_by_fire_key<'a>(
        &'a self,
        schedule_id: i64,
        fire_key: &'a str,
    ) -> ryframe_kernel::AppResult<Option<JobScheduleExecutionRecord>> {
        let execution = self
            .schedule_repository
            .find_execution_by_fire_key(&self.transaction, schedule_id, fire_key)
            .await?;
        execution_record(&self.transaction, execution).await
    }

    async fn insert_schedule(
        &self,
        schedule: JobScheduleRecord,
    ) -> ryframe_kernel::AppResult<JobScheduleRecord> {
        schedule_active(schedule)
            .insert(&self.transaction)
            .await
            .map(to_schedule)
            .map_err(database_error)
    }

    async fn save_schedule(
        &self,
        schedule: JobScheduleRecord,
    ) -> ryframe_kernel::AppResult<JobScheduleRecord> {
        schedule_active(schedule)
            .update(&self.transaction)
            .await
            .map(to_schedule)
            .map_err(database_error)
    }

    async fn insert_execution<'a>(
        &'a self,
        schedule: &'a JobScheduleRecord,
        execution: NewJobScheduleExecution,
    ) -> ryframe_kernel::AppResult<JobScheduleExecutionRecord> {
        let execution = job_schedule_execution::ActiveModel {
            id: Set(execution.id),
            tenant_id: Set(schedule.tenant_id.clone()),
            schedule_id: Set(schedule.id),
            schedule_name_snapshot: Set(schedule.name.clone()),
            handler_key_snapshot: Set(schedule.handler_key.clone()),
            fire_key: Set(execution.fire_key),
            trigger_kind: Set(execution.trigger_kind),
            scheduled_for: Set(execution.scheduled_for),
            outcome: Set(execution.outcome),
            background_job_id: Set(None),
            detail: Set(execution.detail),
            created_at: Set(execution.created_at),
        }
        .insert(&self.transaction)
        .await
        .map_err(database_error)?;
        Ok(to_execution(execution, None))
    }

    async fn attach_background_job(
        &self,
        execution: JobScheduleExecutionRecord,
        background_job_id: i64,
    ) -> ryframe_kernel::AppResult<JobScheduleExecutionRecord> {
        let mut active = execution_active(execution);
        active.background_job_id = Set(Some(background_job_id));
        let execution = active
            .update(&self.transaction)
            .await
            .map_err(database_error)?;
        execution_record(&self.transaction, Some(execution))
            .await?
            .ok_or_else(|| AppError::Internal("调度执行记录更新后丢失".into()))
    }

    async fn enqueue(&self, command: EnqueueJob) -> ryframe_kernel::AppResult<EnqueueJobResult> {
        let now = crate::repositories::database_utc_now(&self.transaction).await?;
        let result = self
            .job_repository
            .enqueue_in_transaction(&self.transaction, database_enqueue(command), now)
            .await?;
        Ok(EnqueueJobResult {
            job_id: result.job.id,
            inserted: result.inserted,
        })
    }
}

#[async_trait::async_trait]
impl ryframe_application::PersistenceTransaction for DatabaseJobScheduleTransaction {
    async fn commit(
        self: Box<Self>,
        audit_mode: ryframe_application::TransactionAuditMode,
    ) -> ryframe_kernel::AppResult<()> {
        let _ = audit_mode;
        self.transaction.commit().await.map_err(database_error)
    }

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        self.transaction.rollback().await.map_err(database_error)
    }
}

fn to_schedule(schedule: job_schedule::Model) -> JobScheduleRecord {
    JobScheduleRecord {
        id: schedule.id,
        tenant_id: schedule.tenant_id,
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
        deleted: schedule.del_flag == job_schedule::Model::DEL_FLAG_DELETED,
    }
}

fn to_execution(
    execution: job_schedule_execution::Model,
    background_job_status: Option<String>,
) -> JobScheduleExecutionRecord {
    JobScheduleExecutionRecord {
        id: execution.id,
        tenant_id: execution.tenant_id,
        schedule_id: execution.schedule_id,
        schedule_name: execution.schedule_name_snapshot,
        handler_key: execution.handler_key_snapshot,
        fire_key: execution.fire_key,
        trigger_kind: execution.trigger_kind,
        scheduled_for: execution.scheduled_for,
        outcome: execution.outcome,
        background_job_id: execution.background_job_id,
        background_job_status,
        detail: execution.detail,
        created_at: execution.created_at,
    }
}

pub fn schedule_active(schedule: JobScheduleRecord) -> job_schedule::ActiveModel {
    job_schedule::ActiveModel {
        id: Set(schedule.id),
        tenant_id: Set(schedule.tenant_id),
        name: Set(schedule.name),
        handler_key: Set(schedule.handler_key),
        cron_expression: Set(schedule.cron_expression),
        timezone: Set(schedule.timezone),
        enabled: Set(schedule.enabled),
        misfire_policy: Set(schedule.misfire_policy),
        concurrency_policy: Set(schedule.concurrency_policy),
        max_runtime_seconds: Set(schedule.max_runtime_seconds),
        next_run_at: Set(schedule.next_run_at),
        last_run_at: Set(schedule.last_run_at),
        version: Set(schedule.version),
        del_flag: Set(if schedule.deleted {
            job_schedule::Model::DEL_FLAG_DELETED.to_owned()
        } else {
            job_schedule::Model::DEL_FLAG_NORMAL.to_owned()
        }),
        created_at: Set(schedule.created_at),
        updated_at: Set(schedule.updated_at),
    }
}

fn execution_active(execution: JobScheduleExecutionRecord) -> job_schedule_execution::ActiveModel {
    job_schedule_execution::ActiveModel {
        id: Set(execution.id),
        tenant_id: Set(execution.tenant_id),
        schedule_id: Set(execution.schedule_id),
        schedule_name_snapshot: Set(execution.schedule_name),
        handler_key_snapshot: Set(execution.handler_key),
        fire_key: Set(execution.fire_key),
        trigger_kind: Set(execution.trigger_kind),
        scheduled_for: Set(execution.scheduled_for),
        outcome: Set(execution.outcome),
        background_job_id: Set(execution.background_job_id),
        detail: Set(execution.detail),
        created_at: Set(execution.created_at),
    }
}

async fn execution_record(
    transaction: &DatabaseTransaction,
    execution: Option<job_schedule_execution::Model>,
) -> ryframe_kernel::AppResult<Option<JobScheduleExecutionRecord>> {
    let Some(execution) = execution else {
        return Ok(None);
    };
    let status = match execution.background_job_id {
        Some(job_id) => background_job::Entity::find_by_id(job_id)
            .one(transaction)
            .await
            .map_err(database_error)?
            .map(|job| job.status),
        None => None,
    };
    Ok(Some(to_execution(execution, status)))
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
