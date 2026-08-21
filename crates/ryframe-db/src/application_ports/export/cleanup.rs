use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, ExportJobRepository, FileRepository, ReadConsistency,
    entities::export_job,
};
use ryframe_kernel::AppError;
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::{
    PersistenceFuture,
    ports::export::{
        ExportCleanupFile, ExportCleanupFileLookup, ExportCleanupPersistencePort,
        ExportCleanupRecord, ExportCleanupTransaction,
    },
};

struct DatabaseExportCleanupPersistence {
    database: ControlDatabaseCluster,
}

struct DatabaseExportCleanupTransaction {
    transaction: DatabaseTransaction,
}

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn ExportCleanupPersistencePort> {
    Arc::new(DatabaseExportCleanupPersistence { database })
}

impl ExportCleanupPersistencePort for DatabaseExportCleanupPersistence {
    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move { crate::repositories::database_utc_now(self.database.write()).await })
    }

    fn list_delete_pending(
        &self,
        after_id: Option<i64>,
        limit: u64,
    ) -> PersistenceFuture<'_, Vec<ExportCleanupRecord>> {
        Box::pin(async move {
            ExportJobRepository
                .list_delete_pending_after_id(self.database.write(), after_id, limit)
                .await
                .map(|records| records.into_iter().map(map_cleanup_record).collect())
        })
    }

    fn list_expired(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        after_id: Option<i64>,
        limit: u64,
    ) -> PersistenceFuture<'_, Vec<ExportCleanupRecord>> {
        Box::pin(async move {
            ExportJobRepository
                .list_expired_succeeded_after_id(self.database.write(), now, after_id, limit)
                .await
                .map(|records| records.into_iter().map(map_cleanup_record).collect())
        })
    }

    fn lookup_result_file<'a>(
        &'a self,
        tenant_id: &'a str,
        export_id: i64,
        file_id: i64,
    ) -> PersistenceFuture<'a, ExportCleanupFileLookup> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            let file = FileRepository
                .find_file_for_purge(&database, tenant_id, file_id)
                .await?;
            if let Some(file) = file {
                return Ok(ExportCleanupFileLookup::Found(ExportCleanupFile {
                    id: file.id,
                    bucket: file.bucket,
                    storage_path: file.storage_path,
                }));
            }
            if ExportJobRepository
                .find_by_id(&database, export_id)
                .await?
                .is_some()
            {
                Ok(ExportCleanupFileLookup::FileMissing)
            } else {
                Ok(ExportCleanupFileLookup::ExportMissing)
            }
        })
    }

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ExportCleanupTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(DatabaseExportCleanupTransaction { transaction })
                as Box<dyn ExportCleanupTransaction>)
        })
    }
}

impl ExportCleanupTransaction for DatabaseExportCleanupTransaction {
    fn lock_export(&self, export_id: i64) -> PersistenceFuture<'_, Option<ExportCleanupRecord>> {
        Box::pin(async move {
            ExportJobRepository
                .find_by_id_for_update_in_transaction(&self.transaction, export_id)
                .await
                .map(|record| record.map(map_cleanup_record))
        })
    }

    fn hard_delete_file<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            FileRepository
                .hard_delete_exclusive_export_file_in_txn(&self.transaction, tenant_id, file_id)
                .await
        })
    }

    fn delete_pending_export(&self, export_id: i64) -> PersistenceFuture<'_, bool> {
        Box::pin(async move {
            ExportJobRepository
                .delete_pending_in_transaction(&self.transaction, export_id)
                .await
        })
    }

    fn mark_expired(
        &self,
        export_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'_, bool> {
        Box::pin(async move {
            ExportJobRepository
                .mark_expired(&self.transaction, export_id, now)
                .await
        })
    }

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.commit().await.map_err(database_error) })
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

fn map_cleanup_record(record: export_job::Model) -> ExportCleanupRecord {
    ExportCleanupRecord {
        id: record.id,
        tenant_id: record.tenant_id,
        status: record.status,
        result_file_id: record.result_file_id,
        expires_at: record.expires_at,
        delete_pending_at: record.delete_pending_at,
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}
