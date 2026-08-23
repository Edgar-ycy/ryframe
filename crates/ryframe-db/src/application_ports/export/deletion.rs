use std::sync::Arc;

use crate::{BackgroundJobRepository, ControlDatabaseCluster, ExportJobRepository};
use ryframe_kernel::AppError;
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::{
    EnqueueJob,
    ports::export::{ExportDeletionPersistencePort, ExportDeletionTransaction},
};

struct DatabaseExportDeletionPersistence {
    database: ControlDatabaseCluster,
}

struct DatabaseExportDeletionTransaction {
    transaction: DatabaseTransaction,
}

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn ExportDeletionPersistencePort> {
    Arc::new(DatabaseExportDeletionPersistence { database })
}

#[async_trait::async_trait]
impl ExportDeletionPersistencePort for DatabaseExportDeletionPersistence {
    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn ExportDeletionTransaction>> {
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        Ok(Box::new(DatabaseExportDeletionTransaction { transaction })
            as Box<dyn ExportDeletionTransaction>)
    }
}

#[async_trait::async_trait]
impl ExportDeletionTransaction for DatabaseExportDeletionTransaction {
    async fn database_now(&self) -> ryframe_kernel::AppResult<chrono::DateTime<chrono::Utc>> {
        crate::repositories::database_utc_now(&self.transaction).await
    }

    async fn mark_delete_pending<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        ids: &'a [i64],
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<u64> {
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
    }

    async fn enqueue_cleanup(
        &self,
        command: EnqueueJob,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<()> {
        BackgroundJobRepository
            .enqueue_in_transaction(
                &self.transaction,
                super::super::jobs::database_enqueue(command),
                now,
            )
            .await
            .map(|_| ())
    }

    async fn commit(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        super::super::audit::commit_current_audit(self.transaction).await
    }

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        self.transaction.rollback().await.map_err(database_error)
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}
