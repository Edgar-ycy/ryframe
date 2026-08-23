use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, ExportJobRepository, FileRepository, ReadConsistency, Repository,
};
use ryframe_kernel::AppError;
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::ports::export::{
    ExportDownloadFile, ExportRequesterPersistencePort, ExportRequesterRecord,
    ExportRequesterTransaction,
};

struct DatabaseExportRequesterPersistence {
    database: ControlDatabaseCluster,
}

struct DatabaseExportRequesterTransaction {
    transaction: DatabaseTransaction,
}

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn ExportRequesterPersistencePort> {
    Arc::new(DatabaseExportRequesterPersistence { database })
}

#[async_trait::async_trait]
impl ExportRequesterPersistencePort for DatabaseExportRequesterPersistence {
    async fn find<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        export_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ExportRequesterRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Strong)
            .connection;
        ExportJobRepository
            .find_by_id_for_requester(&database, tenant_id, requester_id, export_id)
            .await
            .map(|record| record.map(super::mapping::requester_record))
    }

    async fn list_recent<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        limit: u64,
    ) -> ryframe_kernel::AppResult<Vec<ExportRequesterRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Eventual)
            .connection;
        ExportJobRepository
            .list_for_requester(&database, tenant_id, requester_id, limit)
            .await
            .map(|records| {
                records
                    .into_iter()
                    .map(super::mapping::requester_record)
                    .collect()
            })
    }

    async fn list_recent_for_notifications<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        limit: u64,
    ) -> ryframe_kernel::AppResult<Vec<ExportRequesterRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Strong)
            .connection;
        ExportJobRepository
            .list_for_requester(&database, tenant_id, requester_id, limit)
            .await
            .map(|records| {
                records
                    .into_iter()
                    .map(super::mapping::requester_record)
                    .collect()
            })
    }

    async fn database_now(&self) -> ryframe_kernel::AppResult<chrono::DateTime<chrono::Utc>> {
        crate::repositories::database_utc_now(self.database.write()).await
    }

    async fn mark_notifications_read<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        ids: &'a [i64],
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<u64> {
        ExportJobRepository
            .mark_notifications_read(self.database.write(), tenant_id, requester_id, ids, now)
            .await
    }

    async fn find_download_file<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ExportDownloadFile>> {
        FileRepository
            .find_by_id(self.database.write(), tenant_id, file_id)
            .await
            .map(|file| {
                file.map(|file| ExportDownloadFile {
                    bucket: file.bucket,
                    storage_path: file.storage_path,
                })
            })
    }

    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn ExportRequesterTransaction>> {
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        Ok(Box::new(DatabaseExportRequesterTransaction { transaction })
            as Box<dyn ExportRequesterTransaction>)
    }
}

#[async_trait::async_trait]
impl ExportRequesterTransaction for DatabaseExportRequesterTransaction {
    async fn database_now(&self) -> ryframe_kernel::AppResult<chrono::DateTime<chrono::Utc>> {
        crate::repositories::database_utc_now(&self.transaction).await
    }

    async fn cancel<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        export_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        ExportJobRepository
            .cancel_for_requester(&self.transaction, tenant_id, requester_id, export_id, now)
            .await
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
