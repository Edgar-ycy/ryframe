use std::sync::Arc;

use chrono::{DateTime, Utc};
use ryframe_db::{
    ControlDatabaseCluster, ExportJobRepository, FileRepository, ReadConsistency,
    entities::{export_job, sys_file},
};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::TransactionTrait;

use super::{EXPORT_BUCKET, database_error, storage_error};

/// 导出对象、独占文件元数据与公开任务记录的统一清理用例。
pub struct ExportPurgeUseCase {
    db: ControlDatabaseCluster,
    exports: ExportJobRepository,
    files: FileRepository,
    storage: Arc<dyn ryframe_adapters::storage::ObjectStorage>,
}

impl ExportPurgeUseCase {
    pub(crate) fn new(
        db: ControlDatabaseCluster,
        storage: Arc<dyn ryframe_adapters::storage::ObjectStorage>,
    ) -> Self {
        Self {
            db,
            exports: ExportJobRepository,
            files: FileRepository,
            storage,
        }
    }

    /// 清理一条用户删除墓碑；对象删除失败时不改变墓碑，供后台任务可靠重试。
    pub async fn purge_deleted_export(&self, export: export_job::Model) -> AppResult<bool> {
        if export.delete_pending_at.is_none() {
            return Ok(false);
        }
        let file = self.result_file(&export).await?;
        if let Some(file) = &file {
            delete_object_idempotently(self.storage.as_ref(), file).await?;
        }

        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let result = async {
            let Some(current) = self
                .exports
                .find_by_id_for_update_in_transaction(&transaction, export.id)
                .await?
            else {
                return Ok(false);
            };
            if current.delete_pending_at.is_none() {
                return Ok(false);
            }
            if current.tenant_id != export.tenant_id
                || current.result_file_id != export.result_file_id
            {
                return Err(AppError::Conflict("导出任务清理快照已变化".into()));
            }
            if let Some(file) = &file
                && !self
                    .files
                    .hard_delete_exclusive_export_file_in_txn(
                        &transaction,
                        &current.tenant_id,
                        file.id,
                    )
                    .await?
            {
                return Err(AppError::Conflict("导出结果文件元数据已变化".into()));
            }
            self.exports
                .delete_pending_in_transaction(&transaction, current.id)
                .await
        }
        .await;
        finish_transaction(transaction, result).await
    }

    /// 删除到期结果对象与独占文件元数据，并保留一个可供用户删除的 expired 记录。
    pub async fn purge_expired_export(
        &self,
        export: export_job::Model,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let file = self.result_file(&export).await?;
        if let Some(file) = &file {
            delete_object_idempotently(self.storage.as_ref(), file).await?;
        }

        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let result = async {
            let Some(current) = self
                .exports
                .find_by_id_for_update_in_transaction(&transaction, export.id)
                .await?
            else {
                return Ok(false);
            };
            let still_expired = current.delete_pending_at.is_none()
                && current.status == export_job::Model::STATUS_SUCCEEDED
                && current
                    .expires_at
                    .is_some_and(|expires_at| expires_at <= now)
                && current.result_file_id == export.result_file_id;
            if !still_expired {
                return Ok(false);
            }
            if let Some(file) = &file
                && !self
                    .files
                    .hard_delete_exclusive_export_file_in_txn(
                        &transaction,
                        &current.tenant_id,
                        file.id,
                    )
                    .await?
            {
                return Err(AppError::Conflict("导出结果文件元数据已变化".into()));
            }
            self.exports
                .mark_expired(&transaction, current.id, now)
                .await
        }
        .await;
        finish_transaction(transaction, result).await
    }

    async fn result_file(&self, export: &export_job::Model) -> AppResult<Option<sys_file::Model>> {
        let Some(file_id) = export.result_file_id else {
            return Ok(None);
        };
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let file = self
            .files
            .find_file_for_purge(&db, &export.tenant_id, file_id)
            .await?;
        let Some(file) = file else {
            if self.exports.find_by_id(&db, export.id).await?.is_none() {
                return Ok(None);
            }
            return Err(AppError::Conflict(
                "导出任务仍引用结果文件，但文件元数据不存在；已保留清理墓碑".into(),
            ));
        };
        if file.bucket != EXPORT_BUCKET {
            return Err(AppError::Conflict("导出任务引用了非导出结果文件".into()));
        }
        Ok(Some(file))
    }
}

async fn finish_transaction(
    transaction: sea_orm::DatabaseTransaction,
    result: AppResult<bool>,
) -> AppResult<bool> {
    match result {
        Ok(true) => {
            transaction.commit().await.map_err(database_error)?;
            Ok(true)
        }
        Ok(false) => {
            let _ = transaction.rollback().await;
            Ok(false)
        }
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

async fn delete_object_idempotently(
    storage: &dyn ryframe_adapters::storage::ObjectStorage,
    file: &sys_file::Model,
) -> AppResult<()> {
    storage
        .delete(&file.bucket, &file.storage_path)
        .await
        .map_err(storage_error)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use ryframe_adapters::storage::{ObjectStorage, StorageError, StorageResult};

    use super::*;

    struct RetryStorage {
        attempts: AtomicUsize,
    }

    #[async_trait]
    impl ObjectStorage for RetryStorage {
        async fn put(
            &self,
            _bucket: &str,
            _key: &str,
            _data: &[u8],
            _content_type: &str,
        ) -> StorageResult<()> {
            unreachable!("测试不写对象")
        }

        async fn put_file(
            &self,
            _bucket: &str,
            _key: &str,
            _path: &std::path::Path,
            _content_type: &str,
            _sha256_hex: Option<&str>,
        ) -> StorageResult<()> {
            unreachable!("测试不写对象")
        }

        async fn get(&self, _bucket: &str, _key: &str) -> StorageResult<Vec<u8>> {
            unreachable!("测试不读对象")
        }

        async fn get_bounded(
            &self,
            _bucket: &str,
            _key: &str,
            _max_bytes: usize,
        ) -> StorageResult<Vec<u8>> {
            unreachable!("测试不读对象")
        }

        async fn delete(&self, _bucket: &str, _key: &str) -> StorageResult<()> {
            if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(StorageError::Readiness("临时不可用".into()))
            } else {
                Ok(())
            }
        }

        async fn exists(&self, _bucket: &str, _key: &str) -> StorageResult<bool> {
            Ok(false)
        }
    }

    fn file() -> sys_file::Model {
        let now = Utc::now();
        sys_file::Model {
            id: 1,
            tenant_id: "tenant-a".into(),
            original_name: "users-1.xlsx".into(),
            storage_name: "users-1.xlsx".into(),
            storage_path: "tenant-a/exports/users-1.xlsx".into(),
            bucket: EXPORT_BUCKET.into(),
            file_url: "exports/tenant-a/exports/users-1.xlsx".into(),
            file_size: 1,
            content_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                .into(),
            file_sha256: "00".into(),
            upload_by: None,
            upload_status: sys_file::Model::UPLOAD_STATUS_READY.into(),
            reservation_token: None,
            reservation_expires_at: None,
            del_flag: sys_file::Model::DEL_FLAG_NORMAL.into(),
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn storage_failure_keeps_cleanup_retryable() {
        let storage = RetryStorage {
            attempts: AtomicUsize::new(0),
        };
        let artifact = file();
        assert!(
            delete_object_idempotently(&storage, &artifact)
                .await
                .is_err()
        );
        delete_object_idempotently(&storage, &artifact)
            .await
            .expect("重试应能够完成幂等删除");
        assert_eq!(storage.attempts.load(Ordering::SeqCst), 2);
    }
}
