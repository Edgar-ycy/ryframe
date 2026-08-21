use std::sync::Arc;

use crate::{
    BackgroundJobRepository, ControlDatabaseCluster, ExportJobRepository, FileRepository,
    ReadConsistency, Repository, entities::export_job,
};
use ryframe_kernel::AppError;
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::{
    ExportDownloadFile, ExportRequesterPersistencePort, ExportRequesterRecord,
    ExportRequesterTransaction, PersistenceFuture,
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

impl ExportRequesterPersistencePort for DatabaseExportRequesterPersistence {
    fn find<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        export_id: i64,
    ) -> PersistenceFuture<'a, Option<ExportRequesterRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            ExportJobRepository
                .find_by_id_for_requester(&database, tenant_id, requester_id, export_id)
                .await
                .map(|record| record.map(map_export))
        })
    }

    fn list_recent<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        limit: u64,
    ) -> PersistenceFuture<'a, Vec<ExportRequesterRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Eventual)
                .connection;
            ExportJobRepository
                .list_for_requester(&database, tenant_id, requester_id, limit)
                .await
                .map(|records| records.into_iter().map(map_export).collect())
        })
    }

    fn list_recent_for_notifications<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        limit: u64,
    ) -> PersistenceFuture<'a, Vec<ExportRequesterRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            ExportJobRepository
                .list_for_requester(&database, tenant_id, requester_id, limit)
                .await
                .map(|records| records.into_iter().map(map_export).collect())
        })
    }

    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move {
            BackgroundJobRepository
                .database_utc_now(self.database.write())
                .await
        })
    }

    fn mark_notifications_read<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        ids: &'a [i64],
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, u64> {
        Box::pin(async move {
            ExportJobRepository
                .mark_notifications_read(self.database.write(), tenant_id, requester_id, ids, now)
                .await
        })
    }

    fn find_download_file<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> PersistenceFuture<'a, Option<ExportDownloadFile>> {
        Box::pin(async move {
            FileRepository
                .find_by_id(self.database.write(), tenant_id, file_id)
                .await
                .map(|file| {
                    file.map(|file| ExportDownloadFile {
                        bucket: file.bucket,
                        storage_path: file.storage_path,
                    })
                })
        })
    }

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ExportRequesterTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(DatabaseExportRequesterTransaction { transaction })
                as Box<dyn ExportRequesterTransaction>)
        })
    }
}

impl ExportRequesterTransaction for DatabaseExportRequesterTransaction {
    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move {
            BackgroundJobRepository
                .database_utc_now(&self.transaction)
                .await
        })
    }

    fn cancel<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        export_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            ExportJobRepository
                .cancel_for_requester(&self.transaction, tenant_id, requester_id, export_id, now)
                .await
        })
    }

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(
            async move { super::audit_persistence::commit_current_audit(self.transaction).await },
        )
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

pub(crate) fn map_export(record: export_job::Model) -> ExportRequesterRecord {
    ExportRequesterRecord {
        id: record.id,
        resource: record.resource,
        status: record.status,
        result_file_name: record.result_file_name,
        content_type: record.content_type,
        file_size: record.file_size,
        expires_at: record.expires_at,
        error_message: record.error_message,
        snapshot_at: record.snapshot_at,
        matched_rows: record.matched_rows,
        created_at: record.created_at,
        updated_at: record.updated_at,
        completed_at: record.completed_at,
        notification_read_at: record.notification_read_at,
        permission_code: record.permission_code,
        request_params: record.request_params,
        request_version: record.request_version,
        authorization_fingerprint: record.authorization_fingerprint,
        upper_id: record.upper_id,
        result_file_id: record.result_file_id,
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}
