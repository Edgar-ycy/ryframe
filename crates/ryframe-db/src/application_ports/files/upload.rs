use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, FileRepository, Repository, TenantRepository, entities::sys_file,
};
use chrono::{DateTime, Utc};
use ryframe_kernel::AppError;
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::{
    PersistenceTransaction, TransactionAuditMode,
    ports::files::{FileUploadPersistencePort, FileUploadRecord, FileUploadTransaction},
};

struct DatabaseFileUploadPersistence {
    database: ControlDatabaseCluster,
}

struct DatabaseFileUploadTransaction {
    transaction: DatabaseTransaction,
}

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn FileUploadPersistencePort> {
    Arc::new(DatabaseFileUploadPersistence { database })
}

#[async_trait::async_trait]
impl FileUploadPersistencePort for DatabaseFileUploadPersistence {
    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn FileUploadTransaction>> {
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        Ok(Box::new(DatabaseFileUploadTransaction { transaction })
            as Box<dyn FileUploadTransaction>)
    }

    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>> {
        crate::repositories::database_utc_now(self.database.write()).await
    }

    async fn renew_pending<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        reservation_token: &'a str,
        expires_at: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        FileRepository
            .renew_pending_reservation(
                self.database.write(),
                tenant_id,
                file_id,
                reservation_token,
                expires_at,
            )
            .await
    }

    async fn find_any<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> ryframe_kernel::AppResult<Option<FileUploadRecord>> {
        FileRepository
            .find_by_id_any_status(self.database.write(), tenant_id, file_id)
            .await
            .map(|record| record.map(map_record))
    }

    async fn find_ready<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> ryframe_kernel::AppResult<Option<FileUploadRecord>> {
        FileRepository
            .find_by_id(self.database.write(), tenant_id, file_id)
            .await
            .map(|record| record.map(map_record))
    }
}

#[async_trait::async_trait]
impl FileUploadTransaction for DatabaseFileUploadTransaction {
    async fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<()> {
        TenantRepository
            .lock_tenant_in_txn(&self.transaction, tenant_id)
            .await
            .map(|_| ())
    }

    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>> {
        crate::repositories::database_utc_now(&self.transaction).await
    }

    async fn find_by_sha256_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        bucket: &'a str,
        file_sha256: &'a str,
    ) -> ryframe_kernel::AppResult<Option<FileUploadRecord>> {
        FileRepository
            .find_by_sha256_any_status_in_txn(&self.transaction, tenant_id, bucket, file_sha256)
            .await
            .map(|record| record.map(map_record))
    }

    async fn restore_for_reference<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        bucket: &'a str,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        FileRepository
            .restore_file_for_reference_in_txn(&self.transaction, tenant_id, file_id, bucket, now)
            .await
    }

    async fn ensure_storage_quota<'a>(
        &'a self,
        tenant_id: &'a str,
        additional_bytes: u64,
    ) -> ryframe_kernel::AppResult<()> {
        TenantRepository
            .ensure_storage_quota_in_txn(&self.transaction, tenant_id, additional_bytes)
            .await
    }

    async fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: FileUploadRecord,
    ) -> ryframe_kernel::AppResult<FileUploadRecord> {
        FileRepository
            .insert_in_txn(&self.transaction, tenant_id, map_model(record))
            .await
            .map(map_record)
    }

    async fn mark_ready<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        reservation_token: &'a str,
        updated_at: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        FileRepository
            .mark_ready(
                &self.transaction,
                tenant_id,
                file_id,
                reservation_token,
                updated_at,
            )
            .await
    }
}

#[async_trait::async_trait]
impl PersistenceTransaction for DatabaseFileUploadTransaction {
    async fn commit(self: Box<Self>, mode: TransactionAuditMode) -> ryframe_kernel::AppResult<()> {
        match mode {
            TransactionAuditMode::CurrentRequest => {
                super::super::audit::commit_current_audit(self.transaction).await
            }
            TransactionAuditMode::Skip => {
                FileRepository
                    .commit_upload_reservation(self.transaction)
                    .await
            }
        }
    }

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        self.transaction.rollback().await.map_err(database_error)
    }
}

pub fn map_record(file: sys_file::Model) -> FileUploadRecord {
    FileUploadRecord {
        id: file.id,
        tenant_id: file.tenant_id,
        original_name: file.original_name,
        storage_name: file.storage_name,
        storage_path: file.storage_path,
        bucket: file.bucket,
        file_url: file.file_url,
        file_size: file.file_size,
        content_type: file.content_type,
        file_sha256: file.file_sha256,
        upload_by: file.upload_by,
        upload_status: file.upload_status,
        reservation_token: file.reservation_token,
        reservation_expires_at: file.reservation_expires_at,
        del_flag: file.del_flag,
        created_at: file.created_at,
        updated_at: file.updated_at,
    }
}

pub fn map_model(file: FileUploadRecord) -> sys_file::Model {
    sys_file::Model {
        id: file.id,
        tenant_id: file.tenant_id,
        original_name: file.original_name,
        storage_name: file.storage_name,
        storage_path: file.storage_path,
        bucket: file.bucket,
        file_url: file.file_url,
        file_size: file.file_size,
        content_type: file.content_type,
        file_sha256: file.file_sha256,
        upload_by: file.upload_by,
        upload_status: file.upload_status,
        reservation_token: file.reservation_token,
        reservation_expires_at: file.reservation_expires_at,
        del_flag: file.del_flag,
        created_at: file.created_at,
        updated_at: file.updated_at,
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}
