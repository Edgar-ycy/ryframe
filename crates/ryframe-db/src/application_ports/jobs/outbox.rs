use std::sync::Arc;

use crate::{
    BackgroundJobRepository, ControlDatabaseCluster, OutboxEventRepository,
    OutboxFailureDisposition, entities::outbox_event,
};
use chrono::{DateTime, Duration, Utc};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::{
    EnqueueJob, PersistenceFuture,
    ports::jobs::{
        ClaimedOutboxEvent, ExecutionTenantScope, OutboxFailureOutcome, OutboxPersistencePort,
    },
    ports::system::OperLogRecord,
};

use super::{queue::database_enqueue, tenant_scope::database_scope};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn OutboxPersistencePort> {
    Arc::new(DatabaseOutboxPersistence {
        database,
        repository: OutboxEventRepository,
    })
}

struct DatabaseOutboxPersistence {
    database: ControlDatabaseCluster,
    repository: OutboxEventRepository,
}

impl OutboxPersistencePort for DatabaseOutboxPersistence {
    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>> {
        Box::pin(async move { crate::repositories::database_utc_now(self.database.write()).await })
    }

    fn claim_next<'a>(
        &'a self,
        worker_id: &'a str,
        lease_duration: Duration,
        now: DateTime<Utc>,
        tenant_scope: &'a ExecutionTenantScope,
    ) -> PersistenceFuture<'a, Option<ClaimedOutboxEvent>> {
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
                .map(|event| event.map(to_claimed_event))
        })
    }

    fn publish_background_job<'a>(
        &'a self,
        event_id: i64,
        worker_id: &'a str,
        command: EnqueueJob,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            let transaction = begin(self.database.write()).await?;
            let result = async {
                BackgroundJobRepository
                    .enqueue_in_transaction(&transaction, database_enqueue(command), now)
                    .await?;
                self.repository
                    .mark_published_in_transaction(&transaction, event_id, worker_id, now)
                    .await
            }
            .await;
            finish(transaction, result).await
        })
    }

    fn publish_audit<'a>(
        &'a self,
        event_id: i64,
        worker_id: &'a str,
        tenant_id: &'a str,
        record: OperLogRecord,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            let transaction = begin(self.database.write()).await?;
            let result = async {
                super::super::system::insert_oper_log(&transaction, tenant_id, record)
                    .await
                    .inspect_err(|_| ryframe_application::record_audit_failure("oper_log_write"))?;
                self.repository
                    .mark_published_in_transaction(&transaction, event_id, worker_id, now)
                    .await
            }
            .await;
            finish(transaction, result).await
        })
    }

    fn mark_published<'a>(
        &'a self,
        event_id: i64,
        worker_id: &'a str,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            let transaction = begin(self.database.write()).await?;
            let marked = self
                .repository
                .mark_published_in_transaction(&transaction, event_id, worker_id, now)
                .await;
            finish(transaction, marked).await
        })
    }

    fn fail<'a>(
        &'a self,
        event_id: i64,
        worker_id: &'a str,
        retry_at: DateTime<Utc>,
        error_message: &'a str,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, OutboxFailureOutcome> {
        Box::pin(async move {
            self.repository
                .fail(
                    self.database.write(),
                    event_id,
                    worker_id,
                    retry_at,
                    error_message,
                    now,
                )
                .await
                .map(to_failure_outcome)
        })
    }

    fn recover_expired_leases<'a>(
        &'a self,
        now: DateTime<Utc>,
        tenant_scope: &'a ExecutionTenantScope,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            self.repository
                .recover_expired_leases(self.database.write(), now, &database_scope(tenant_scope))
                .await
        })
    }
}

async fn begin(database: &sea_orm::DatabaseConnection) -> AppResult<DatabaseTransaction> {
    database
        .begin()
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

async fn finish(transaction: DatabaseTransaction, result: AppResult<bool>) -> AppResult<bool> {
    match result {
        Ok(true) => {
            transaction
                .commit()
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
            Ok(true)
        }
        Ok(false) => {
            let _ = transaction.rollback().await;
            Ok(false)
        }
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

pub fn to_claimed_event(event: outbox_event::Model) -> ClaimedOutboxEvent {
    ClaimedOutboxEvent {
        id: event.id,
        tenant_id: event.tenant_id,
        event_type: event.event_type,
        payload: event.payload,
        attempts: event.attempts,
        max_attempts: event.max_attempts,
        dedupe_key: event.dedupe_key,
        traceparent: event.traceparent,
        tracestate: event.tracestate,
    }
}

fn to_failure_outcome(value: OutboxFailureDisposition) -> OutboxFailureOutcome {
    match value {
        OutboxFailureDisposition::Retried { available_at } => {
            OutboxFailureOutcome::Retried { available_at }
        }
        OutboxFailureDisposition::Dead => OutboxFailureOutcome::Dead,
        OutboxFailureDisposition::LeaseLost => OutboxFailureOutcome::LeaseLost,
    }
}
