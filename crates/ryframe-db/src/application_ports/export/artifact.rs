use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, ExportJobRepository, FileRepository, MarkExportJobSucceeded,
    entities::sys_file,
};
use ryframe_kernel::AppError;
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::ports::export::{
    CompleteExportArtifact, ExportArtifactFileDraft, ExportArtifactFileRecord,
    ExportArtifactPersistencePort, ExportArtifactState, ExportArtifactTransaction,
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

#[async_trait::async_trait]
impl ExportArtifactPersistencePort for DatabaseExportArtifactPersistence {
    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn ExportArtifactTransaction>> {
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        Ok(Box::new(DatabaseExportArtifactTransaction { transaction })
            as Box<dyn ExportArtifactTransaction>)
    }
}

#[async_trait::async_trait]
impl ExportArtifactTransaction for DatabaseExportArtifactTransaction {
    async fn database_now(&self) -> ryframe_kernel::AppResult<chrono::DateTime<chrono::Utc>> {
        crate::repositories::database_utc_now(&self.transaction).await
    }

    async fn lock_export(
        &self,
        export_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ExportArtifactState>> {
        ExportJobRepository
            .find_by_id_for_update_in_transaction(&self.transaction, export_id)
            .await
            .map(|job| {
                job.map(|job| ExportArtifactState {
                    status: job.status,
                    result_file_id: job.result_file_id,
                })
            })
    }

    async fn insert_ready_file<'a>(
        &'a self,
        tenant_id: &'a str,
        file: ExportArtifactFileDraft,
    ) -> ryframe_kernel::AppResult<ExportArtifactFileRecord> {
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
    }

    async fn mark_succeeded(
        &self,
        command: CompleteExportArtifact,
    ) -> ryframe_kernel::AppResult<bool> {
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
