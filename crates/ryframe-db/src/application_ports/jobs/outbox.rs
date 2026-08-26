use crate::DbResultExt;
use std::sync::Arc;

use crate::{
    BackgroundJobRepository, ControlDatabaseCluster, OutboxEventRepository,
    OutboxFailureDisposition, entities::outbox_event,
};
use chrono::{DateTime, Duration, Utc};
use ryframe_kernel::AppResult;
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::{
    EnqueueJob,
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

#[async_trait::async_trait]
impl OutboxPersistencePort for DatabaseOutboxPersistence {
    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>> {
        crate::repositories::database_utc_now(self.database.write()).await
    }

    async fn claim_next<'a>(
        &'a self,
        worker_id: &'a str,
        lease_duration: Duration,
        now: DateTime<Utc>,
        tenant_scope: &'a ExecutionTenantScope,
    ) -> ryframe_kernel::AppResult<Option<ClaimedOutboxEvent>> {
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
    }

    async fn publish_background_job<'a>(
        &'a self,
        event_id: i64,
        worker_id: &'a str,
        command: EnqueueJob,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
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
    }

    async fn publish_audit<'a>(
        &'a self,
        event_id: i64,
        worker_id: &'a str,
        tenant_id: &'a str,
        record: OperLogRecord,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
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
    }

    async fn mark_published<'a>(
        &'a self,
        event_id: i64,
        worker_id: &'a str,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        let transaction = begin(self.database.write()).await?;
        let marked = self
            .repository
            .mark_published_in_transaction(&transaction, event_id, worker_id, now)
            .await;
        finish(transaction, marked).await
    }

    async fn fail<'a>(
        &'a self,
        event_id: i64,
        worker_id: &'a str,
        retry_at: DateTime<Utc>,
        error_message: &'a str,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<OutboxFailureOutcome> {
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
    }

    async fn recover_expired_leases<'a>(
        &'a self,
        now: DateTime<Utc>,
        tenant_scope: &'a ExecutionTenantScope,
    ) -> ryframe_kernel::AppResult<()> {
        self.repository
            .recover_expired_leases(self.database.write(), now, &database_scope(tenant_scope))
            .await
    }
}

async fn begin(database: &sea_orm::DatabaseConnection) -> AppResult<DatabaseTransaction> {
    database.begin().await.db()
}

async fn finish(transaction: DatabaseTransaction, result: AppResult<bool>) -> AppResult<bool> {
    match result {
        Ok(true) => {
            transaction.commit().await.db()?;
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
