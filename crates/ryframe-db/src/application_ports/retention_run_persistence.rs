use std::sync::Arc;

use crate::{ControlDatabaseCluster, DataRetentionRepository, entities::data_retention_run};
use ryframe_kernel::{AppError, PageResult, ValidatedPageQuery};
use sea_orm::TransactionTrait;

use super::control_transaction::DatabasePortTransaction;

use ryframe_application::{
    PersistenceFuture, RetentionRunPersistencePort, RetentionRunRecord, RetentionRunTransaction,
    ports::jobs::BackgroundJobTransaction,
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn RetentionRunPersistencePort> {
    Arc::new(DatabaseRetentionRunPersistence {
        database,
        repository: DataRetentionRepository,
    })
}

struct DatabaseRetentionRunPersistence {
    database: ControlDatabaseCluster,
    repository: DataRetentionRepository,
}

struct DatabaseRetentionRunTransaction {
    transaction: DatabasePortTransaction,
}

impl RetentionRunPersistencePort for DatabaseRetentionRunPersistence {
    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move {
            self.repository
                .database_utc_now(self.database.write())
                .await
        })
    }

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn RetentionRunTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(DatabaseRetentionRunTransaction {
                transaction: transaction.into(),
            }) as Box<dyn RetentionRunTransaction>)
        })
    }

    fn insert_if_missing(
        &self,
        record: RetentionRunRecord,
    ) -> PersistenceFuture<'_, RetentionRunRecord> {
        Box::pin(async move {
            self.repository
                .insert_run_if_missing(self.database.write(), to_model(record))
                .await
                .map(to_record)
        })
    }

    fn update(&self, record: RetentionRunRecord) -> PersistenceFuture<'_, RetentionRunRecord> {
        Box::pin(async move {
            self.repository
                .update_run(self.database.write(), to_model(record))
                .await
                .map(to_record)
        })
    }

    fn list(
        &self,
        page: ValidatedPageQuery,
    ) -> PersistenceFuture<'_, PageResult<RetentionRunRecord>> {
        Box::pin(async move {
            let result = self
                .repository
                .list_runs(self.database.write(), &page)
                .await?;
            Ok(PageResult::new(
                result.records.into_iter().map(to_record).collect(),
                result.total,
                &page,
            ))
        })
    }
}

impl RetentionRunTransaction for DatabaseRetentionRunTransaction {
    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move {
            DataRetentionRepository
                .database_utc_now(&self.transaction)
                .await
        })
    }

    fn background_jobs(&self) -> &dyn BackgroundJobTransaction {
        &self.transaction
    }

    fn find_by_background_job(
        &self,
        background_job_id: i64,
    ) -> PersistenceFuture<'_, Option<RetentionRunRecord>> {
        Box::pin(async move {
            DataRetentionRepository
                .find_run_by_background_job(&self.transaction, background_job_id)
                .await
                .map(|record| record.map(to_record))
        })
    }

    fn insert_if_missing(
        &self,
        record: RetentionRunRecord,
    ) -> PersistenceFuture<'_, RetentionRunRecord> {
        Box::pin(async move {
            DataRetentionRepository
                .insert_run_if_missing(&self.transaction, to_model(record))
                .await
                .map(to_record)
        })
    }

    fn lock_by_background_job(
        &self,
        background_job_id: i64,
    ) -> PersistenceFuture<'_, Option<RetentionRunRecord>> {
        Box::pin(async move {
            DataRetentionRepository
                .lock_run_by_background_job_in_txn(&self.transaction, background_job_id)
                .await
                .map(|record| record.map(to_record))
        })
    }

    fn begin_run(
        &self,
        record: RetentionRunRecord,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'_, Option<RetentionRunRecord>> {
        Box::pin(async move {
            DataRetentionRepository
                .begin_run_in_txn(&self.transaction, to_model(record), now)
                .await
                .map(|record| record.map(to_record))
        })
    }

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move {
            super::audit_persistence::commit_current_audit(self.transaction.into_inner()).await
        })
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

fn to_record(model: data_retention_run::Model) -> RetentionRunRecord {
    RetentionRunRecord {
        id: model.id,
        background_job_id: model.background_job_id,
        trigger_kind: model.trigger_kind,
        status: model.status,
        policy_snapshot: model.policy_snapshot,
        eligible_counts: model.eligible_counts,
        deleted_counts: model.deleted_counts,
        remaining_counts: model.remaining_counts,
        requested_by: model.requested_by,
        error_summary: model.error_summary,
        started_at: model.started_at,
        completed_at: model.completed_at,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

fn to_model(record: RetentionRunRecord) -> data_retention_run::Model {
    data_retention_run::Model {
        id: record.id,
        background_job_id: record.background_job_id,
        trigger_kind: record.trigger_kind,
        status: record.status,
        policy_snapshot: record.policy_snapshot,
        eligible_counts: record.eligible_counts,
        deleted_counts: record.deleted_counts,
        remaining_counts: record.remaining_counts,
        requested_by: record.requested_by,
        error_summary: record.error_summary,
        started_at: record.started_at,
        completed_at: record.completed_at,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn run_mapping_preserves_state() {
        let now = Utc.with_ymd_and_hms(2026, 8, 21, 2, 3, 4).unwrap();
        let record = RetentionRunRecord {
            id: 1,
            background_job_id: 2,
            trigger_kind: RetentionRunRecord::TRIGGER_MANUAL.into(),
            status: RetentionRunRecord::STATUS_RUNNING.into(),
            policy_snapshot: serde_json::json!({"policy": 1}),
            eligible_counts: serde_json::json!({"files": 2}),
            deleted_counts: serde_json::json!({"files": 1}),
            remaining_counts: serde_json::json!({"files": 1}),
            requested_by: Some(3),
            error_summary: None,
            started_at: Some(now),
            completed_at: None,
            created_at: now,
            updated_at: now,
        };

        assert_eq!(to_record(to_model(record)).background_job_id, 2);
    }
}
