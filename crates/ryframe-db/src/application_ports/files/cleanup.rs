use std::sync::Arc;

use crate::{ControlDatabaseCluster, FileRepository, TenantRepository, entities::sys_file};
use chrono::{DateTime, Utc};
use ryframe_kernel::AppError;
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::ports::files::{
    FileCleanupPersistencePort, FileCleanupRecord, FileCleanupTransaction,
};

struct DatabaseFileCleanupPersistence {
    database: ControlDatabaseCluster,
}

struct DatabaseFileCleanupTransaction {
    transaction: DatabaseTransaction,
}

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn FileCleanupPersistencePort> {
    Arc::new(DatabaseFileCleanupPersistence { database })
}

#[async_trait::async_trait]
impl FileCleanupPersistencePort for DatabaseFileCleanupPersistence {
    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn FileCleanupTransaction>> {
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        Ok(Box::new(DatabaseFileCleanupTransaction { transaction })
            as Box<dyn FileCleanupTransaction>)
    }

    async fn find<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> ryframe_kernel::AppResult<Option<FileCleanupRecord>> {
        FileRepository
            .find_by_id_any_status(self.database.write(), tenant_id, file_id)
            .await
            .map(|record| record.map(map_record))
    }

    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>> {
        crate::repositories::database_utc_now(self.database.write()).await
    }

    async fn find_stale_config_packages(
        &self,
        ready_before: DateTime<Utc>,
        limit: u64,
    ) -> ryframe_kernel::AppResult<Vec<FileCleanupRecord>> {
        FileRepository
            .find_stale_unreferenced_config_packages(self.database.write(), ready_before, limit)
            .await
            .map(|records| records.into_iter().map(map_record).collect())
    }

    async fn find_expired_reservations(
        &self,
        now: DateTime<Utc>,
        limit: u64,
    ) -> ryframe_kernel::AppResult<Vec<FileCleanupRecord>> {
        FileRepository
            .find_expired_reservations(self.database.write(), now, limit)
            .await
            .map(|records| records.into_iter().map(map_record).collect())
    }

    async fn begin_expired_cleanup<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        now: DateTime<Utc>,
        cleanup_after: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        FileRepository
            .begin_expired_cleanup(
                self.database.write(),
                tenant_id,
                file_id,
                now,
                cleanup_after,
            )
            .await
    }

    async fn claim_expired_cleanup<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        claim_token: &'a str,
        claimed_at: DateTime<Utc>,
        claim_until: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        FileRepository
            .claim_expired_cleanup(
                self.database.write(),
                tenant_id,
                file_id,
                claim_token,
                claimed_at,
                claim_until,
            )
            .await
    }

    async fn begin_owned_cleanup<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        reservation_token: &'a str,
        cleanup_after: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        FileRepository
            .begin_cleanup(
                self.database.write(),
                tenant_id,
                file_id,
                reservation_token,
                cleanup_after,
            )
            .await
    }

    async fn defer_claim<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        claim_token: &'a str,
        updated_at: DateTime<Utc>,
        retry_at: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        FileRepository
            .defer_cleanup_claim(
                self.database.write(),
                tenant_id,
                file_id,
                claim_token,
                updated_at,
                retry_at,
            )
            .await
    }

    async fn complete_claim<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        claim_token: &'a str,
    ) -> ryframe_kernel::AppResult<bool> {
        FileRepository
            .complete_cleanup_claim(self.database.write(), tenant_id, file_id, claim_token)
            .await
    }
}

#[async_trait::async_trait]
impl FileCleanupTransaction for DatabaseFileCleanupTransaction {
    async fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<()> {
        TenantRepository
            .lock_tenant_in_txn(&self.transaction, tenant_id)
            .await
            .map(|_| ())
    }

    async fn find_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> ryframe_kernel::AppResult<Option<FileCleanupRecord>> {
        FileRepository
            .find_by_id_any_status_for_update(&self.transaction, tenant_id, file_id)
            .await
            .map(|record| record.map(map_record))
    }

    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>> {
        crate::repositories::database_utc_now(&self.transaction).await
    }

    async fn claim_expired_import<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        claim_token: &'a str,
        expired_before: DateTime<Utc>,
        claim_until: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        FileRepository
            .claim_ready_expired_import_artifact_in_txn(
                &self.transaction,
                tenant_id,
                file_id,
                claim_token,
                expired_before,
                claim_until,
            )
            .await
    }

    async fn mark_unreferenced_config_package<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        now: DateTime<Utc>,
        cleanup_after: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        FileRepository
            .mark_unreferenced_config_package_for_cleanup_in_txn(
                &self.transaction,
                tenant_id,
                file_id,
                now,
                cleanup_after,
            )
            .await
    }

    async fn commit(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        self.transaction.commit().await.map_err(database_error)
    }

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        self.transaction.rollback().await.map_err(database_error)
    }
}

fn map_record(file: sys_file::Model) -> FileCleanupRecord {
    FileCleanupRecord {
        id: file.id,
        tenant_id: file.tenant_id,
        bucket: file.bucket,
        storage_path: file.storage_path,
        upload_status: file.upload_status,
        reservation_token: file.reservation_token,
        reservation_expires_at: file.reservation_expires_at,
        del_flag: file.del_flag,
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}
