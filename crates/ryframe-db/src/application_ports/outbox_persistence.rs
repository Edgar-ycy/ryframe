use std::sync::Arc;

use crate::{
    BackgroundJobRepository, ControlDatabaseCluster, OutboxEventRepository,
    OutboxFailureDisposition, entities::outbox_event,
};
use chrono::{DateTime, Duration, Utc};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::{
    ClaimedOutboxEvent, EnqueueJob, ExecutionTenantScope, OperLogRecord, OutboxFailureOutcome,
    OutboxPersistencePort, PersistenceFuture,
};

use super::{execution_tenant_scope::database_scope, job_queue_persistence::database_enqueue};

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
                super::oper_log_persistence::insert_event_in_transaction(
                    &transaction,
                    tenant_id,
                    record,
                )
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

fn to_claimed_event(event: outbox_event::Model) -> ClaimedOutboxEvent {
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

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;

    #[test]
    fn claimed_event_mapping_keeps_worker_fields() {
        let now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let event = outbox_event::Model {
            id: 41,
            tenant_id: Some("tenant-a".into()),
            event_type: "system.message.published".into(),
            aggregate_type: "message".into(),
            aggregate_id: "99".into(),
            payload: json!({"message_id": 99}),
            status: outbox_event::Model::STATUS_RUNNING.into(),
            available_at: now,
            attempts: 2,
            max_attempts: 5,
            lease_owner: Some("worker-a".into()),
            lease_until: Some(now + Duration::seconds(30)),
            dedupe_key: Some("message:99".into()),
            traceparent: Some("trace".into()),
            tracestate: Some("state".into()),
            last_error: None,
            published_at: None,
            created_at: now,
            updated_at: now,
        };

        let claimed = to_claimed_event(event);

        assert_eq!(claimed.id, 41);
        assert_eq!(claimed.tenant_id.as_deref(), Some("tenant-a"));
        assert_eq!(claimed.attempts, 2);
        assert_eq!(claimed.max_attempts, 5);
        assert_eq!(claimed.dedupe_key.as_deref(), Some("message:99"));
        assert_eq!(claimed.traceparent.as_deref(), Some("trace"));
        assert_eq!(claimed.tracestate.as_deref(), Some("state"));
    }
}
