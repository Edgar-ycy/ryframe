use std::sync::Arc;

use ryframe_db::{BackgroundJobRepository, ControlDatabaseCluster, ExportJobRepository};
use ryframe_kernel::AppError;
use sea_orm::{DatabaseTransaction, TransactionTrait};

use crate::{
    EnqueueJob, ExportDeletionPersistencePort, ExportDeletionTransaction, PersistenceFuture,
};

struct LegacyExportDeletionPersistence {
    database: ControlDatabaseCluster,
}

struct LegacyExportDeletionTransaction {
    transaction: DatabaseTransaction,
}

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn ExportDeletionPersistencePort> {
    Arc::new(LegacyExportDeletionPersistence { database })
}

impl ExportDeletionPersistencePort for LegacyExportDeletionPersistence {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ExportDeletionTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(LegacyExportDeletionTransaction { transaction })
                as Box<dyn ExportDeletionTransaction>)
        })
    }
}

impl ExportDeletionTransaction for LegacyExportDeletionTransaction {
    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move {
            BackgroundJobRepository
                .database_utc_now(&self.transaction)
                .await
        })
    }

    fn mark_delete_pending<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        ids: &'a [i64],
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, u64> {
        Box::pin(async move {
            ExportJobRepository
                .mark_delete_pending_in_transaction(
                    &self.transaction,
                    tenant_id,
                    requester_id,
                    ids,
                    now,
                )
                .await
                .map(|result| result.removed_unread_count)
        })
    }

    fn enqueue_cleanup(
        &self,
        command: EnqueueJob,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'_, ()> {
        Box::pin(async move {
            BackgroundJobRepository
                .enqueue_in_transaction(
                    &self.transaction,
                    crate::legacy_job_queue_persistence::database_enqueue(command),
                    now,
                )
                .await
                .map(|_| ())
        })
    }

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { crate::commit_current_audit(self.transaction).await })
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}
