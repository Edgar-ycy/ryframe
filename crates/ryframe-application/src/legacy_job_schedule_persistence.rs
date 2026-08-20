use std::sync::Arc;

use ryframe_db::{
    ControlDatabaseCluster, JobScheduleExecutionFilter, JobScheduleFilter, JobScheduleRepository,
    entities::{job_schedule, job_schedule_execution},
};
use ryframe_kernel::{PageResult, ValidatedPageQuery};

use crate::{
    JobScheduleExecutionReadFilter, JobScheduleExecutionRecord, JobScheduleReadFilter,
    JobScheduleReadPort, JobScheduleRecord, PersistenceFuture,
};

pub fn read_port(database: ControlDatabaseCluster) -> Arc<dyn JobScheduleReadPort> {
    Arc::new(LegacyJobSchedulePersistence {
        database,
        repository: JobScheduleRepository,
    })
}

struct LegacyJobSchedulePersistence {
    database: ControlDatabaseCluster,
    repository: JobScheduleRepository,
}

impl JobScheduleReadPort for LegacyJobSchedulePersistence {
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

fn to_schedule(schedule: job_schedule::Model) -> JobScheduleRecord {
    JobScheduleRecord {
        id: schedule.id,
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

fn to_execution(
    execution: job_schedule_execution::Model,
    background_job_status: Option<String>,
) -> JobScheduleExecutionRecord {
    JobScheduleExecutionRecord {
        id: execution.id,
        schedule_id: execution.schedule_id,
        schedule_name: execution.schedule_name_snapshot,
        handler_key: execution.handler_key_snapshot,
        trigger_kind: execution.trigger_kind,
        scheduled_for: execution.scheduled_for,
        outcome: execution.outcome,
        background_job_id: execution.background_job_id,
        background_job_status,
        detail: execution.detail,
        created_at: execution.created_at,
    }
}
