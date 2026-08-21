use std::sync::Arc;

use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};

use super::{EXPORT_BUCKET, EXPORT_STATUS_SUCCEEDED, storage_error};
use crate::ports::{
    export::{
        ExportCleanupFile, ExportCleanupFileLookup, ExportCleanupPersistencePort,
        ExportCleanupRecord, ExportCleanupTransaction,
    },
    files::ArtifactStore,
};

/// 导出对象、独占文件元数据与公开任务记录的统一清理用例。
pub(super) struct ExportPurgeUseCase {
    persistence: Arc<dyn ExportCleanupPersistencePort>,
    storage: Arc<dyn ArtifactStore>,
}

impl ExportPurgeUseCase {
    pub(crate) fn new(
        persistence: Arc<dyn ExportCleanupPersistencePort>,
        storage: Arc<dyn ArtifactStore>,
    ) -> Self {
        Self {
            persistence,
            storage,
        }
    }

    /// 清理一条用户删除墓碑；对象删除失败时不改变墓碑，供后台任务可靠重试。
    pub async fn purge_deleted_export(&self, export: ExportCleanupRecord) -> AppResult<bool> {
        if export.delete_pending_at.is_none() {
            return Ok(false);
        }
        let file = self.result_file(&export).await?;
        if let Some(file) = &file {
            delete_object_idempotently(self.storage.as_ref(), file).await?;
        }

        let transaction = self.persistence.begin().await?;
        let result = async {
            let Some(current) = transaction.lock_export(export.id).await? else {
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
                && !transaction
                    .hard_delete_file(&current.tenant_id, file.id)
                    .await?
            {
                return Err(AppError::Conflict("导出结果文件元数据已变化".into()));
            }
            transaction.delete_pending_export(current.id).await
        }
        .await;
        finish_transaction(transaction, result).await
    }

    /// 删除到期结果对象与独占文件元数据，并保留一个可供用户删除的 expired 记录。
    pub async fn purge_expired_export(
        &self,
        export: ExportCleanupRecord,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let file = self.result_file(&export).await?;
        if let Some(file) = &file {
            delete_object_idempotently(self.storage.as_ref(), file).await?;
        }

        let transaction = self.persistence.begin().await?;
        let result = async {
            let Some(current) = transaction.lock_export(export.id).await? else {
                return Ok(false);
            };
            let still_expired = current.delete_pending_at.is_none()
                && current.status == EXPORT_STATUS_SUCCEEDED
                && current
                    .expires_at
                    .is_some_and(|expires_at| expires_at <= now)
                && current.result_file_id == export.result_file_id;
            if !still_expired {
                return Ok(false);
            }
            if let Some(file) = &file
                && !transaction
                    .hard_delete_file(&current.tenant_id, file.id)
                    .await?
            {
                return Err(AppError::Conflict("导出结果文件元数据已变化".into()));
            }
            transaction.mark_expired(current.id, now).await
        }
        .await;
        finish_transaction(transaction, result).await
    }

    async fn result_file(
        &self,
        export: &ExportCleanupRecord,
    ) -> AppResult<Option<ExportCleanupFile>> {
        let Some(file_id) = export.result_file_id else {
            return Ok(None);
        };
        let lookup = self
            .persistence
            .lookup_result_file(&export.tenant_id, export.id, file_id)
            .await?;
        let file = match lookup {
            ExportCleanupFileLookup::ExportMissing => return Ok(None),
            ExportCleanupFileLookup::FileMissing => {
                return Err(AppError::Conflict(
                    "导出任务仍引用结果文件，但文件元数据不存在；已保留清理墓碑".into(),
                ));
            }
            ExportCleanupFileLookup::Found(file) => file,
        };
        if file.bucket != EXPORT_BUCKET {
            return Err(AppError::Conflict("导出任务引用了非导出结果文件".into()));
        }
        Ok(Some(file))
    }
}

async fn finish_transaction(
    transaction: Box<dyn ExportCleanupTransaction>,
    result: AppResult<bool>,
) -> AppResult<bool> {
    match result {
        Ok(true) => {
            transaction.commit().await?;
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
    storage: &dyn ArtifactStore,
    file: &ExportCleanupFile,
) -> AppResult<()> {
    storage
        .delete(&file.bucket, &file.storage_path)
        .await
        .map_err(storage_error)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::ports::files::{ArtifactStoreError, ArtifactStoreErrorKind, ArtifactStoreFuture};

    use super::*;

    struct RetryStorage {
        attempts: AtomicUsize,
    }

    impl ArtifactStore for RetryStorage {
        fn readiness<'a>(&'a self, _bucket: &'a str) -> ArtifactStoreFuture<'a, ()> {
            unreachable!("测试不检查存储")
        }

        fn ensure_bucket<'a>(&'a self, _bucket: &'a str) -> ArtifactStoreFuture<'a, ()> {
            unreachable!("测试不创建桶")
        }

        fn put<'a>(
            &'a self,
            _bucket: &'a str,
            _key: &'a str,
            _data: &'a [u8],
            _content_type: &'a str,
        ) -> ArtifactStoreFuture<'a, ()> {
            unreachable!("测试不写对象")
        }

        fn put_file<'a>(
            &'a self,
            _bucket: &'a str,
            _key: &'a str,
            _path: &'a std::path::Path,
            _content_type: &'a str,
            _sha256_hex: Option<&'a str>,
        ) -> ArtifactStoreFuture<'a, ()> {
            unreachable!("测试不写对象")
        }

        fn get<'a>(&'a self, _bucket: &'a str, _key: &'a str) -> ArtifactStoreFuture<'a, Vec<u8>> {
            unreachable!("测试不读对象")
        }

        fn delete<'a>(&'a self, _bucket: &'a str, _key: &'a str) -> ArtifactStoreFuture<'a, ()> {
            Box::pin(async move {
                if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(ArtifactStoreError::new(
                        ArtifactStoreErrorKind::Unavailable,
                        "临时不可用",
                    ))
                } else {
                    Ok(())
                }
            })
        }
    }

    fn file() -> ExportCleanupFile {
        ExportCleanupFile {
            id: 1,
            storage_path: "tenant-a/exports/users-1.xlsx".into(),
            bucket: EXPORT_BUCKET.into(),
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
