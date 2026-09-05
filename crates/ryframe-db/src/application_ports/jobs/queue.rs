use crate::DbResultExt;
use std::sync::Arc;

use crate::{
    BackgroundJobFilter as DatabaseJobFilter, BackgroundJobRepository,
    BackgroundJobStats as DatabaseJobStats, BackgroundJobTypeStats as DatabaseTypeStats,
    ControlDatabaseCluster, EnqueueBackgroundJob, EnqueueBackgroundJobResult, FailBackgroundJob,
    JobFailureDisposition as DatabaseFailureOutcome,
    entities::{
        background_job,
        tenant::{
            config_bundle as tenant_config_bundle, config_transfer as tenant_config_transfer,
        },
    },
};
use chrono::{DateTime, Duration, Utc};
use ryframe_kernel::{PageResult, ValidatedPageQuery};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use super::super::transaction::DatabasePortTransaction;

use ryframe_application::{
    EnqueueJob, EnqueueJobResult,
    ports::jobs::{
        BackgroundJobPersistencePort, BackgroundJobReadFilter, BackgroundJobRecord,
        BackgroundJobStatsRecord, BackgroundJobTransaction, BackgroundJobTypeStats,
        ClaimedJobRecord, ExecutionTenantScope, FailJobCommand, JobFailureOutcome,
        RecoveredJobLeases, TenantConfigJobKind,
    },
};

use super::tenant_scope::database_scope;

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn BackgroundJobPersistencePort> {
    Arc::new(DatabaseJobQueuePersistence {
        database,
        repository: BackgroundJobRepository,
    })
}

struct DatabaseJobQueuePersistence {
    database: ControlDatabaseCluster,
    repository: BackgroundJobRepository,
}

#[async_trait::async_trait]
impl BackgroundJobPersistencePort for DatabaseJobQueuePersistence {
    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>> {
        crate::repositories::database_utc_now(self.database.write()).await
    }

    async fn claim_next<'a>(
        &'a self,
        worker_id: &'a str,
        lease_duration: Duration,
        now: DateTime<Utc>,
        tenant_scope: &'a ExecutionTenantScope,
    ) -> ryframe_kernel::AppResult<Option<ClaimedJobRecord>> {
        self.repository
            .claim_next(
                self.database.write(),
                worker_id,
                lease_duration,
                now,
                &database_scope(tenant_scope),
            )
            .await
            .map(|job| job.map(to_claimed_record))
    }

    async fn dead_letter<'a>(
        &'a self,
        job_id: i64,
        worker_id: &'a str,
        error_message: &'a str,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<JobFailureOutcome> {
        self.repository
            .dead_letter(self.database.write(), job_id, worker_id, error_message, now)
            .await
            .map(to_failure_outcome)
    }

    async fn renew_lease<'a>(
        &'a self,
        job_id: i64,
        worker_id: &'a str,
        lease_duration: Duration,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        self.repository
            .renew_lease(
                self.database.write(),
                job_id,
                worker_id,
                lease_duration,
                now,
            )
            .await
    }

    async fn complete<'a>(
        &'a self,
        job_id: i64,
        worker_id: &'a str,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        self.repository
            .complete(self.database.write(), job_id, worker_id, now)
            .await
    }

    async fn defer_retryable_conflict<'a>(
        &'a self,
        job_id: i64,
        worker_id: &'a str,
        available_at: DateTime<Utc>,
        error_message: &'a str,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<JobFailureOutcome> {
        self.repository
            .defer_retryable_conflict(
                self.database.write(),
                job_id,
                worker_id,
                available_at,
                error_message,
                now,
            )
            .await
            .map(to_failure_outcome)
    }

    async fn fail<'a>(
        &'a self,
        command: FailJobCommand<'a>,
    ) -> ryframe_kernel::AppResult<JobFailureOutcome> {
        self.repository
            .fail(
                self.database.write(),
                FailBackgroundJob {
                    job_id: command.job_id,
                    worker_id: command.worker_id,
                    retry_at: command.retry_at,
                    error_message: command.error_message,
                    force_dead: command.force_dead,
                    now: command.now,
                },
            )
            .await
            .map(to_failure_outcome)
    }

    async fn stats_for_types<'a>(
        &'a self,
        job_types: &'a [String],
        tenant_scope: &'a ExecutionTenantScope,
    ) -> ryframe_kernel::AppResult<Vec<BackgroundJobTypeStats>> {
        self.repository
            .stats_for_types(
                self.database.write(),
                job_types,
                &database_scope(tenant_scope),
            )
            .await
            .map(|stats| stats.into_iter().map(to_type_stats).collect())
    }

    async fn recover_expired_leases<'a>(
        &'a self,
        now: DateTime<Utc>,
        tenant_scope: &'a ExecutionTenantScope,
    ) -> ryframe_kernel::AppResult<RecoveredJobLeases> {
        self.repository
            .recover_expired_leases(self.database.write(), now, &database_scope(tenant_scope))
            .await
            .map(|value| RecoveredJobLeases {
                requeued: value.requeued,
                dead: value.dead,
                completed: value.completed,
            })
    }

    async fn enqueue(&self, command: EnqueueJob) -> ryframe_kernel::AppResult<EnqueueJobResult> {
        let now = crate::repositories::database_utc_now(self.database.write()).await?;
        self.repository
            .enqueue(self.database.write(), database_enqueue(command), now)
            .await
            .map(to_enqueue_result)
    }

    async fn list<'a>(
        &'a self,
        filter: BackgroundJobReadFilter<'a>,
        page: ValidatedPageQuery,
    ) -> ryframe_kernel::AppResult<PageResult<BackgroundJobRecord>> {
        let result = self
            .repository
            .list(self.database.write(), database_filter(filter), &page)
            .await?;
        Ok(PageResult::new(
            result.records.into_iter().map(to_job_record).collect(),
            result.total,
            &page,
        ))
    }

    async fn stats<'a>(
        &'a self,
        filter: BackgroundJobReadFilter<'a>,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<BackgroundJobStatsRecord> {
        self.repository
            .stats_filtered(self.database.write(), database_filter(filter), now)
            .await
            .map(to_stats_record)
    }

    async fn find_for_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
        include_platform: bool,
        job_id: i64,
    ) -> ryframe_kernel::AppResult<Option<BackgroundJobRecord>> {
        self.repository
            .find_by_id_for_tenant(self.database.write(), tenant_id, include_platform, job_id)
            .await
            .map(|job| job.map(to_job_record))
    }

    async fn retry_dead<'a>(
        &'a self,
        tenant_id: &'a str,
        include_platform: bool,
        job_id: i64,
        retried_by: i64,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        self.repository
            .retry_dead(
                self.database.write(),
                tenant_id,
                include_platform,
                job_id,
                retried_by,
                now,
            )
            .await
    }

    async fn tenant_config_job_owner<'a>(
        &'a self,
        tenant_id: &'a str,
        job_id: i64,
        kind: TenantConfigJobKind,
    ) -> ryframe_kernel::AppResult<Option<i64>> {
        match kind {
            TenantConfigJobKind::Export => tenant_config_bundle::Entity::find()
                .filter(tenant_config_bundle::Column::TenantId.eq(tenant_id))
                .filter(tenant_config_bundle::Column::BackgroundJobId.eq(job_id))
                .one(self.database.write())
                .await
                .db()
                .map(|bundle| bundle.map(|bundle| bundle.created_by)),
            TenantConfigJobKind::Preview
            | TenantConfigJobKind::Apply
            | TenantConfigJobKind::Rollback => {
                transfer_job_owner(self.database.write(), tenant_id, job_id, kind).await
            }
        }
    }
}

#[async_trait::async_trait]
impl BackgroundJobTransaction for DatabasePortTransaction {
    async fn enqueue(&self, command: EnqueueJob) -> ryframe_kernel::AppResult<EnqueueJobResult> {
        let now = crate::repositories::database_utc_now(self).await?;
        BackgroundJobRepository
            .enqueue_in_transaction(self, database_enqueue(command), now)
            .await
            .map(to_enqueue_result)
    }

    async fn reactivate_linked<'a>(
        &'a self,
        job_id: i64,
        expected_job_type: &'a str,
        payload_key: &'a str,
        expected_resource_id: i64,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        BackgroundJobRepository
            .reactivate_linked_in_txn(
                self,
                job_id,
                expected_job_type,
                payload_key,
                expected_resource_id,
                now,
            )
            .await
    }
}

pub fn database_enqueue(command: EnqueueJob) -> EnqueueBackgroundJob {
    EnqueueBackgroundJob {
        tenant_id: command.tenant_id,
        schedule_id: command.schedule_id,
        scheduled_for: command.scheduled_for,
        max_runtime_seconds: command.max_runtime_seconds,
        job_type: command.job_type,
        payload: command.payload,
        priority: command.priority,
        available_at: command.available_at,
        max_attempts: command.max_attempts,
        dedupe_key: command.dedupe_key,
        traceparent: command.traceparent,
        tracestate: command.tracestate,
    }
}

fn to_enqueue_result(result: EnqueueBackgroundJobResult) -> EnqueueJobResult {
    EnqueueJobResult {
        job_id: result.job.id,
        inserted: result.inserted,
    }
}

fn database_filter(filter: BackgroundJobReadFilter<'_>) -> DatabaseJobFilter<'_> {
    DatabaseJobFilter {
        tenant_id: filter.tenant_id,
        include_platform: filter.include_platform,
        schedule_id: filter.schedule_id,
        job_type: filter.job_type,
        status: filter.status,
    }
}

fn to_claimed_record(job: background_job::Model) -> ClaimedJobRecord {
    ClaimedJobRecord {
        id: job.id,
        tenant_id: job.tenant_id,
        job_type: job.job_type,
        payload: job.payload,
        lease_owner: job.lease_owner,
        attempts: job.attempts,
        max_attempts: job.max_attempts,
        max_runtime_seconds: job.max_runtime_seconds,
        traceparent: job.traceparent,
        tracestate: job.tracestate,
    }
}

pub fn to_job_record(job: background_job::Model) -> BackgroundJobRecord {
    BackgroundJobRecord {
        id: job.id,
        tenant_id: job.tenant_id,
        schedule_id: job.schedule_id,
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
        last_error: job.last_error,
        created_at: job.created_at,
        updated_at: job.updated_at,
        completed_at: job.completed_at,
    }
}

fn to_failure_outcome(value: DatabaseFailureOutcome) -> JobFailureOutcome {
    match value {
        DatabaseFailureOutcome::Retried { available_at } => {
            JobFailureOutcome::Retried { available_at }
        }
        DatabaseFailureOutcome::Dead => JobFailureOutcome::Dead,
        DatabaseFailureOutcome::Completed => JobFailureOutcome::Completed,
        DatabaseFailureOutcome::LeaseLost => JobFailureOutcome::LeaseLost,
    }
}

fn to_stats_record(value: DatabaseJobStats) -> BackgroundJobStatsRecord {
    BackgroundJobStatsRecord {
        total: value.total,
        pending: value.pending,
        running: value.running,
        succeeded: value.succeeded,
        dead: value.dead,
        ready: value.ready,
    }
}

fn to_type_stats(value: DatabaseTypeStats) -> BackgroundJobTypeStats {
    BackgroundJobTypeStats {
        job_type: value.job_type,
        pending: value.pending,
        running: value.running,
        dead: value.dead,
        ready: value.ready,
        oldest_ready_age: value.oldest_ready_age,
    }
}

async fn transfer_job_owner(
    db: &sea_orm::DatabaseConnection,
    tenant_id: &str,
    job_id: i64,
    kind: TenantConfigJobKind,
) -> ryframe_kernel::AppResult<Option<i64>> {
    let query = tenant_config_transfer::Entity::find()
        .filter(tenant_config_transfer::Column::TenantId.eq(tenant_id));
    let query = match kind {
        TenantConfigJobKind::Preview => {
            query.filter(tenant_config_transfer::Column::PreviewBackgroundJobId.eq(job_id))
        }
        TenantConfigJobKind::Apply => {
            query.filter(tenant_config_transfer::Column::ApplyBackgroundJobId.eq(job_id))
        }
        TenantConfigJobKind::Rollback => {
            query.filter(tenant_config_transfer::Column::RollbackBackgroundJobId.eq(job_id))
        }
        TenantConfigJobKind::Export => unreachable!("导出任务使用配置包表查询"),
    };
    query
        .one(db)
        .await
        .db()
        .map(|transfer| transfer.map(|transfer| transfer.requested_by))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_failure_outcomes_keep_terminal_meaning() {
        let retry_at = Utc::now();
        for (database, application) in [
            (
                DatabaseFailureOutcome::Retried {
                    available_at: retry_at,
                },
                JobFailureOutcome::Retried {
                    available_at: retry_at,
                },
            ),
            (DatabaseFailureOutcome::Dead, JobFailureOutcome::Dead),
            (
                DatabaseFailureOutcome::Completed,
                JobFailureOutcome::Completed,
            ),
            (
                DatabaseFailureOutcome::LeaseLost,
                JobFailureOutcome::LeaseLost,
            ),
        ] {
            assert_eq!(to_failure_outcome(database), application);
        }
    }
}
