use std::sync::Arc;

use crate::{
    BackgroundJobFilter as DatabaseJobFilter, BackgroundJobRepository,
    BackgroundJobStats as DatabaseJobStats, BackgroundJobTypeStats as DatabaseTypeStats,
    ControlDatabaseCluster, EnqueueBackgroundJob, EnqueueBackgroundJobResult, FailBackgroundJob,
    JobFailureDisposition as DatabaseFailureOutcome,
    entities::{background_job, tenant_config_bundle, tenant_config_transfer},
};
use chrono::{DateTime, Duration, Utc};
use ryframe_kernel::{PageResult, ValidatedPageQuery};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use super::super::control_transaction::DatabasePortTransaction;

use ryframe_application::{
    EnqueueJob, EnqueueJobResult, PersistenceFuture,
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

impl BackgroundJobPersistencePort for DatabaseJobQueuePersistence {
    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>> {
        Box::pin(async move {
            self.repository
                .database_utc_now(self.database.write())
                .await
        })
    }

    fn claim_next<'a>(
        &'a self,
        worker_id: &'a str,
        lease_duration: Duration,
        now: DateTime<Utc>,
        tenant_scope: &'a ExecutionTenantScope,
    ) -> PersistenceFuture<'a, Option<ClaimedJobRecord>> {
        Box::pin(async move {
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
        })
    }

    fn dead_letter<'a>(
        &'a self,
        job_id: i64,
        worker_id: &'a str,
        error_message: &'a str,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            self.repository
                .dead_letter(self.database.write(), job_id, worker_id, error_message, now)
                .await
        })
    }

    fn renew_lease<'a>(
        &'a self,
        job_id: i64,
        worker_id: &'a str,
        lease_duration: Duration,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            self.repository
                .renew_lease(
                    self.database.write(),
                    job_id,
                    worker_id,
                    lease_duration,
                    now,
                )
                .await
        })
    }

    fn complete<'a>(
        &'a self,
        job_id: i64,
        worker_id: &'a str,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            self.repository
                .complete(self.database.write(), job_id, worker_id, now)
                .await
        })
    }

    fn defer_retryable_conflict<'a>(
        &'a self,
        job_id: i64,
        worker_id: &'a str,
        available_at: DateTime<Utc>,
        error_message: &'a str,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
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
        })
    }

    fn fail<'a>(&'a self, command: FailJobCommand<'a>) -> PersistenceFuture<'a, JobFailureOutcome> {
        Box::pin(async move {
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
        })
    }

    fn stats_for_types<'a>(
        &'a self,
        job_types: &'a [String],
        tenant_scope: &'a ExecutionTenantScope,
    ) -> PersistenceFuture<'a, Vec<BackgroundJobTypeStats>> {
        Box::pin(async move {
            self.repository
                .stats_for_types(
                    self.database.write(),
                    job_types,
                    &database_scope(tenant_scope),
                )
                .await
                .map(|stats| stats.into_iter().map(to_type_stats).collect())
        })
    }

    fn recover_expired_leases<'a>(
        &'a self,
        now: DateTime<Utc>,
        tenant_scope: &'a ExecutionTenantScope,
    ) -> PersistenceFuture<'a, RecoveredJobLeases> {
        Box::pin(async move {
            self.repository
                .recover_expired_leases(self.database.write(), now, &database_scope(tenant_scope))
                .await
                .map(|value| RecoveredJobLeases {
                    requeued: value.requeued,
                    dead: value.dead,
                })
        })
    }

    fn enqueue(&self, command: EnqueueJob) -> PersistenceFuture<'_, EnqueueJobResult> {
        Box::pin(async move {
            let now = self
                .repository
                .database_utc_now(self.database.write())
                .await?;
            self.repository
                .enqueue(self.database.write(), database_enqueue(command), now)
                .await
                .map(to_enqueue_result)
        })
    }

    fn list<'a>(
        &'a self,
        filter: BackgroundJobReadFilter<'a>,
        page: ValidatedPageQuery,
    ) -> PersistenceFuture<'a, PageResult<BackgroundJobRecord>> {
        Box::pin(async move {
            let result = self
                .repository
                .list(self.database.write(), database_filter(filter), &page)
                .await?;
            Ok(PageResult::new(
                result.records.into_iter().map(to_job_record).collect(),
                result.total,
                &page,
            ))
        })
    }

    fn stats<'a>(
        &'a self,
        filter: BackgroundJobReadFilter<'a>,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, BackgroundJobStatsRecord> {
        Box::pin(async move {
            self.repository
                .stats_filtered(self.database.write(), database_filter(filter), now)
                .await
                .map(to_stats_record)
        })
    }

    fn find_for_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
        include_platform: bool,
        job_id: i64,
    ) -> PersistenceFuture<'a, Option<BackgroundJobRecord>> {
        Box::pin(async move {
            self.repository
                .find_by_id_for_tenant(self.database.write(), tenant_id, include_platform, job_id)
                .await
                .map(|job| job.map(to_job_record))
        })
    }

    fn retry_dead<'a>(
        &'a self,
        tenant_id: &'a str,
        include_platform: bool,
        job_id: i64,
        retried_by: i64,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
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
        })
    }

    fn tenant_config_job_owner<'a>(
        &'a self,
        tenant_id: &'a str,
        job_id: i64,
        kind: TenantConfigJobKind,
    ) -> PersistenceFuture<'a, Option<i64>> {
        Box::pin(async move {
            match kind {
                TenantConfigJobKind::Export => tenant_config_bundle::Entity::find()
                    .filter(tenant_config_bundle::Column::TenantId.eq(tenant_id))
                    .filter(tenant_config_bundle::Column::BackgroundJobId.eq(job_id))
                    .one(self.database.write())
                    .await
                    .map_err(database_error)
                    .map(|bundle| bundle.map(|bundle| bundle.created_by)),
                TenantConfigJobKind::Preview
                | TenantConfigJobKind::Apply
                | TenantConfigJobKind::Rollback => {
                    transfer_job_owner(self.database.write(), tenant_id, job_id, kind).await
                }
            }
        })
    }
}

impl BackgroundJobTransaction for DatabasePortTransaction {
    fn enqueue(&self, command: EnqueueJob) -> PersistenceFuture<'_, EnqueueJobResult> {
        Box::pin(async move {
            let now = BackgroundJobRepository.database_utc_now(self).await?;
            BackgroundJobRepository
                .enqueue_in_transaction(self, database_enqueue(command), now)
                .await
                .map(to_enqueue_result)
        })
    }

    fn reactivate_linked<'a>(
        &'a self,
        job_id: i64,
        expected_job_type: &'a str,
        payload_key: &'a str,
        expected_resource_id: i64,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
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
        })
    }
}

pub(crate) fn database_enqueue(command: EnqueueJob) -> EnqueueBackgroundJob {
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

fn to_job_record(job: background_job::Model) -> BackgroundJobRecord {
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
        .map_err(database_error)
        .map(|transfer| transfer.map(|transfer| transfer.requested_by))
}

fn database_error(error: impl std::fmt::Display) -> ryframe_kernel::AppError {
    ryframe_kernel::AppError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn enqueue_mapping_moves_every_command_field() {
        let now = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
        let command = database_enqueue(EnqueueJob {
            tenant_id: Some("tenant-a".into()),
            schedule_id: Some(8),
            scheduled_for: Some(now),
            max_runtime_seconds: Some(30),
            job_type: "test.job".into(),
            payload: serde_json::json!({"id": "9"}),
            priority: 4,
            available_at: now,
            max_attempts: 5,
            dedupe_key: Some("dedupe".into()),
            traceparent: Some("traceparent".into()),
            tracestate: Some("tracestate".into()),
        });

        assert_eq!(command.tenant_id.as_deref(), Some("tenant-a"));
        assert_eq!(command.schedule_id, Some(8));
        assert_eq!(command.scheduled_for, Some(now));
        assert_eq!(command.max_runtime_seconds, Some(30));
        assert_eq!(command.job_type, "test.job");
        assert_eq!(command.payload, serde_json::json!({"id": "9"}));
        assert_eq!(command.priority, 4);
        assert_eq!(command.available_at, now);
        assert_eq!(command.max_attempts, 5);
        assert_eq!(command.dedupe_key.as_deref(), Some("dedupe"));
        assert_eq!(command.traceparent.as_deref(), Some("traceparent"));
        assert_eq!(command.tracestate.as_deref(), Some("tracestate"));
    }

    #[test]
    fn persistence_mapping_keeps_job_fields() {
        let now = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
        let record = to_job_record(background_job::Model {
            id: 9,
            tenant_id: Some("tenant-a".into()),
            schedule_id: Some(8),
            scheduled_for: Some(now),
            max_runtime_seconds: Some(30),
            job_type: "job.test".into(),
            payload: serde_json::json!({"key": "value"}),
            status: background_job::Model::STATUS_RUNNING.into(),
            priority: 3,
            available_at: now,
            attempts: 2,
            max_attempts: 5,
            lease_owner: Some("worker-a".into()),
            lease_until: Some(now),
            dedupe_key: Some("dedupe".into()),
            traceparent: Some("trace".into()),
            tracestate: Some("state".into()),
            last_error: Some("error".into()),
            created_at: now,
            updated_at: now,
            completed_at: None,
        });

        assert_eq!(record.id, 9);
        assert_eq!(record.tenant_id.as_deref(), Some("tenant-a"));
        assert_eq!(record.schedule_id, Some(8));
        assert_eq!(record.job_type, "job.test");
        assert_eq!(record.attempts, 2);
        assert_eq!(record.max_attempts, 5);
        assert_eq!(record.last_error.as_deref(), Some("error"));
    }
}
