use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, ExportJobRepository, FileRepository, ReadConsistency,
    entities::export_job,
};
use ryframe_kernel::AppError;
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::ports::export::{
    ExportCleanupFile, ExportCleanupFileLookup, ExportCleanupPersistencePort, ExportCleanupRecord,
    ExportCleanupTransaction,
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

#[async_trait::async_trait]
impl ExportCleanupPersistencePort for DatabaseExportCleanupPersistence {
    async fn database_now(&self) -> ryframe_kernel::AppResult<chrono::DateTime<chrono::Utc>> {
        crate::repositories::database_utc_now(self.database.write()).await
    }

    async fn list_delete_pending(
        &self,
        after_id: Option<i64>,
        limit: u64,
    ) -> ryframe_kernel::AppResult<Vec<ExportCleanupRecord>> {
        ExportJobRepository
            .list_delete_pending_after_id(self.database.write(), after_id, limit)
            .await
            .map(|records| records.into_iter().map(map_cleanup_record).collect())
    }

    async fn list_expired(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        after_id: Option<i64>,
        limit: u64,
    ) -> ryframe_kernel::AppResult<Vec<ExportCleanupRecord>> {
        ExportJobRepository
            .list_expired_succeeded_after_id(self.database.write(), now, after_id, limit)
            .await
            .map(|records| records.into_iter().map(map_cleanup_record).collect())
    }

    async fn lookup_result_file<'a>(
        &'a self,
        tenant_id: &'a str,
        export_id: i64,
        file_id: i64,
    ) -> ryframe_kernel::AppResult<ExportCleanupFileLookup> {
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
    }

    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn ExportCleanupTransaction>> {
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        Ok(Box::new(DatabaseExportCleanupTransaction { transaction })
            as Box<dyn ExportCleanupTransaction>)
    }
}

#[async_trait::async_trait]
impl ExportCleanupTransaction for DatabaseExportCleanupTransaction {
    async fn lock_export(
        &self,
        export_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ExportCleanupRecord>> {
        ExportJobRepository
            .find_by_id_for_update_in_transaction(&self.transaction, export_id)
            .await
            .map(|record| record.map(map_cleanup_record))
    }

    async fn hard_delete_file<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> ryframe_kernel::AppResult<bool> {
        FileRepository
            .hard_delete_exclusive_export_file_in_txn(&self.transaction, tenant_id, file_id)
            .await
    }

    async fn delete_pending_export(&self, export_id: i64) -> ryframe_kernel::AppResult<bool> {
        ExportJobRepository
            .delete_pending_in_transaction(&self.transaction, export_id)
            .await
    }

    async fn mark_expired(
        &self,
        export_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        ExportJobRepository
            .mark_expired(&self.transaction, export_id, now)
            .await
    }
}

#[async_trait::async_trait]
impl ryframe_application::PersistenceTransaction for DatabaseExportCleanupTransaction {
    async fn commit(
        self: Box<Self>,
        audit_mode: ryframe_application::TransactionAuditMode,
    ) -> ryframe_kernel::AppResult<()> {
        let _ = audit_mode;
        self.transaction.commit().await.map_err(database_error)
    }

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        self.transaction.rollback().await.map_err(database_error)
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
