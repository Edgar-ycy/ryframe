use std::sync::Arc;

use crate::{
    BackgroundJobRepository, ControlDatabaseCluster, ExportJobRepository, FileRepository,
    MarkExportJobSucceeded, entities::sys_file,
};
use ryframe_kernel::AppError;
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::{
    PersistenceFuture,
    ports::export::{
        CompleteExportArtifact, ExportArtifactFileDraft, ExportArtifactFileRecord,
        ExportArtifactPersistencePort, ExportArtifactState, ExportArtifactTransaction,
    },
};

struct DatabaseExportArtifactPersistence {
    database: ControlDatabaseCluster,
}

struct DatabaseExportArtifactTransaction {
    transaction: DatabaseTransaction,
}

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn ExportArtifactPersistencePort> {
    Arc::new(DatabaseExportArtifactPersistence { database })
}

impl ExportArtifactPersistencePort for DatabaseExportArtifactPersistence {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ExportArtifactTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(DatabaseExportArtifactTransaction { transaction })
                as Box<dyn ExportArtifactTransaction>)
        })
    }
}

impl ExportArtifactTransaction for DatabaseExportArtifactTransaction {
    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move {
            BackgroundJobRepository
                .database_utc_now(&self.transaction)
                .await
        })
    }

    fn lock_export(&self, export_id: i64) -> PersistenceFuture<'_, Option<ExportArtifactState>> {
        Box::pin(async move {
            ExportJobRepository
                .find_by_id_for_update_in_transaction(&self.transaction, export_id)
                .await
                .map(|job| {
                    job.map(|job| ExportArtifactState {
                        status: job.status,
                        result_file_id: job.result_file_id,
                    })
                })
        })
    }

    fn insert_ready_file<'a>(
        &'a self,
        tenant_id: &'a str,
        file: ExportArtifactFileDraft,
    ) -> PersistenceFuture<'a, ExportArtifactFileRecord> {
        Box::pin(async move {
            let model = sys_file::Model {
                id: file.id,
                tenant_id: tenant_id.to_owned(),
                original_name: file.file_name.clone(),
                storage_name: file.file_name,
                storage_path: file.storage_path,
                bucket: file.bucket,
                file_url: file.file_url,
                file_size: file.file_size,
                content_type: file.content_type,
                file_sha256: file.sha256,
                upload_by: Some(file.uploaded_by),
                upload_status: sys_file::Model::UPLOAD_STATUS_READY.into(),
                reservation_token: None,
                reservation_expires_at: None,
                del_flag: sys_file::Model::DEL_FLAG_NORMAL.into(),
                created_at: file.created_at,
                updated_at: file.created_at,
            };
            FileRepository
                .insert_in_txn(&self.transaction, tenant_id, model)
                .await
                .map(|file| ExportArtifactFileRecord {
                    id: file.id,
                    file_name: file.original_name,
                    content_type: file.content_type,
                    file_size: file.file_size,
                })
        })
    }

    fn mark_succeeded(&self, command: CompleteExportArtifact) -> PersistenceFuture<'_, bool> {
        Box::pin(async move {
            ExportJobRepository
                .mark_succeeded_in_transaction(
                    &self.transaction,
                    MarkExportJobSucceeded {
                        id: command.export_id,
                        file_id: command.file_id,
                        file_name: command.file_name,
                        content_type: command.content_type,
                        file_size: command.file_size,
                        expires_at: command.expires_at,
                        completed_at: command.completed_at,
                    },
                )
                .await
        })
    }

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { super::super::audit::commit_current_audit(self.transaction).await })
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}
