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
    EnqueueJob, EnqueueJobResult, PersistenceFuture,
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

impl JobScheduleReadPort for DatabaseJobSchedulePersistence {
    fn page<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: JobScheduleReadFilter<'a>,
        page: ValidatedPageQuery,
    ) -> PersistenceFuture<'a, PageResult<JobScheduleRecord>> {
        Box::pin(async move {
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
        })
    }

    fn find<'a>(
        &'a self,
        tenant_id: &'a str,
        schedule_id: i64,
    ) -> PersistenceFuture<'a, Option<JobScheduleRecord>> {
        Box::pin(async move {
            Ok(self
                .repository
                .find_for_tenant(self.database.write(), tenant_id, schedule_id)
                .await?
                .map(to_schedule))
        })
    }

    fn execution_page<'a>(
        &'a self,
        tenant_id: &'a str,
        schedule_id: i64,
        filter: JobScheduleExecutionReadFilter<'a>,
        page: ValidatedPageQuery,
    ) -> PersistenceFuture<'a, PageResult<JobScheduleExecutionRecord>> {
        Box::pin(async move {
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
        })
    }
}

impl JobSchedulePersistencePort for DatabaseJobSchedulePersistence {
    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move {
            BackgroundJobRepository
                .database_utc_now(self.database.write())
                .await
        })
    }

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn JobScheduleTransaction>> {
        Box::pin(async move {
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
        })
    }
}

struct DatabaseJobScheduleTransaction {
    transaction: DatabaseTransaction,
    schedule_repository: JobScheduleRepository,
    job_repository: BackgroundJobRepository,
}

impl JobScheduleTransaction for DatabaseJobScheduleTransaction {
    fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
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
        })
    }

    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move {
            self.job_repository
                .database_utc_now(&self.transaction)
                .await
        })
    }

    fn count_enabled<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, u64> {
        Box::pin(async move {
            self.schedule_repository
                .count_enabled(&self.transaction, tenant_id)
                .await
        })
    }

    fn lock_schedule<'a>(
        &'a self,
        tenant_id: &'a str,
        schedule_id: i64,
    ) -> PersistenceFuture<'a, Option<JobScheduleRecord>> {
        Box::pin(async move {
            Ok(self
                .schedule_repository
                .lock_for_tenant(&self.transaction, tenant_id, schedule_id)
                .await?
                .map(to_schedule))
        })
    }

    fn lock_next_due<'a>(
        &'a self,
        now: chrono::DateTime<chrono::Utc>,
        tenant_scope: &'a ExecutionTenantScope,
    ) -> PersistenceFuture<'a, Option<JobScheduleRecord>> {
        Box::pin(async move {
            let tenant_scope = database_scope(tenant_scope);
            Ok(self
                .schedule_repository
                .lock_next_due(&self.transaction, now, &tenant_scope)
                .await?
                .map(to_schedule))
        })
    }

    fn has_active_job(&self, schedule_id: i64) -> PersistenceFuture<'_, bool> {
        Box::pin(async move {
            self.schedule_repository
                .has_active_job(&self.transaction, schedule_id)
                .await
        })
    }

    fn find_execution_by_fire_key<'a>(
        &'a self,
        schedule_id: i64,
        fire_key: &'a str,
    ) -> PersistenceFuture<'a, Option<JobScheduleExecutionRecord>> {
        Box::pin(async move {
            let execution = self
                .schedule_repository
                .find_execution_by_fire_key(&self.transaction, schedule_id, fire_key)
                .await?;
            execution_record(&self.transaction, execution).await
        })
    }

    fn insert_schedule(
        &self,
        schedule: JobScheduleRecord,
    ) -> PersistenceFuture<'_, JobScheduleRecord> {
        Box::pin(async move {
            schedule_active(schedule)
                .insert(&self.transaction)
                .await
                .map(to_schedule)
                .map_err(database_error)
        })
    }

    fn save_schedule(
        &self,
        schedule: JobScheduleRecord,
    ) -> PersistenceFuture<'_, JobScheduleRecord> {
        Box::pin(async move {
            schedule_active(schedule)
                .update(&self.transaction)
                .await
                .map(to_schedule)
                .map_err(database_error)
        })
    }

    fn insert_execution<'a>(
        &'a self,
        schedule: &'a JobScheduleRecord,
        execution: NewJobScheduleExecution,
    ) -> PersistenceFuture<'a, JobScheduleExecutionRecord> {
        Box::pin(async move {
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
        })
    }

    fn attach_background_job(
        &self,
        execution: JobScheduleExecutionRecord,
        background_job_id: i64,
    ) -> PersistenceFuture<'_, JobScheduleExecutionRecord> {
        Box::pin(async move {
            let mut active = execution_active(execution);
            active.background_job_id = Set(Some(background_job_id));
            let execution = active
                .update(&self.transaction)
                .await
                .map_err(database_error)?;
            execution_record(&self.transaction, Some(execution))
                .await?
                .ok_or_else(|| AppError::Internal("调度执行记录更新后丢失".into()))
        })
    }

    fn enqueue(&self, command: EnqueueJob) -> PersistenceFuture<'_, EnqueueJobResult> {
        Box::pin(async move {
            let now = self
                .job_repository
                .database_utc_now(&self.transaction)
                .await?;
            let result = self
                .job_repository
                .enqueue_in_transaction(&self.transaction, database_enqueue(command), now)
                .await?;
            Ok(EnqueueJobResult {
                job_id: result.job.id,
                inserted: result.inserted,
            })
        })
    }

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.commit().await.map_err(database_error) })
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
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

fn schedule_active(schedule: JobScheduleRecord) -> job_schedule::ActiveModel {
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

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use sea_orm::ActiveValue::Set;

    use super::{JobScheduleRecord, job_schedule, schedule_active};

    #[test]
    fn schedule_mapping_preserves_state_and_deletion() {
        let now = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
        let active = schedule_active(JobScheduleRecord {
            id: 42,
            tenant_id: "tenant-a".to_owned(),
            name: "清理任务".to_owned(),
            handler_key: "system.cleanup".to_owned(),
            cron_expression: "0 0 0 * * * *".to_owned(),
            timezone: "UTC".to_owned(),
            enabled: false,
            misfire_policy: "skip".to_owned(),
            concurrency_policy: "forbid".to_owned(),
            max_runtime_seconds: 600,
            next_run_at: None,
            last_run_at: Some(now),
            version: 5,
            created_at: now,
            updated_at: now,
            deleted: true,
        });

        assert_eq!(active.id, Set(42));
        assert_eq!(active.tenant_id, Set("tenant-a".to_owned()));
        assert_eq!(active.version, Set(5));
        assert_eq!(
            active.del_flag,
            Set(job_schedule::Model::DEL_FLAG_DELETED.to_owned())
        );
        assert_eq!(active.last_run_at, Set(Some(now)));
    }
}
