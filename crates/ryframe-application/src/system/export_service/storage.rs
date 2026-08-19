use ryframe_db::{
    MarkExportJobSucceeded,
    entities::{export_job, sys_file},
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::TransactionTrait;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use super::*;

impl ExportService {
    pub(super) async fn persist_export_file(
        &self,
        export: export_job::Model,
        actor: ActorContext,
        artifact: ryframe_adapters::excel::ExcelArtifact,
        resource: &str,
    ) -> AppResult<()> {
        let (file_name, key) = export_file_location(&export.tenant_id, resource, export.id);
        let file_id = deterministic_export_file_id(export.id);
        let content_type =
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_owned();
        let file_size = i64::try_from(artifact.size())
            .map_err(|_| AppError::PayloadTooLarge("导出文件超过数据库大小范围".into()))?;
        let file_sha256 = hash_file(artifact.path()).await?;
        self.storage
            .ensure_bucket(EXPORT_BUCKET)
            .await
            .map_err(storage_error)?;
        if let Err(error) = self
            .storage
            .put_file(
                EXPORT_BUCKET,
                &key,
                artifact.path(),
                &content_type,
                Some(&file_sha256),
            )
            .await
        {
            self.delete_uncommitted_object(&key).await;
            return Err(storage_error(error));
        }
        let now = match self.background_jobs.database_utc_now(self.db.write()).await {
            Ok(now) => now,
            Err(error) => {
                self.delete_uncommitted_object(&key).await;
                return Err(error);
            }
        };
        let file = sys_file::Model {
            id: file_id,
            tenant_id: export.tenant_id.clone(),
            original_name: file_name.clone(),
            storage_name: file_name.clone(),
            storage_path: key.clone(),
            bucket: EXPORT_BUCKET.into(),
            file_url: format!("{EXPORT_BUCKET}/{key}"),
            file_size,
            content_type: content_type.clone(),
            file_sha256,
            upload_by: Some(actor.username),
            upload_status: sys_file::Model::UPLOAD_STATUS_READY.into(),
            reservation_token: None,
            reservation_expires_at: None,
            del_flag: sys_file::Model::DEL_FLAG_NORMAL.into(),
            created_at: now,
            updated_at: now,
        };
        let transaction = match self.db.write().begin().await {
            Ok(transaction) => transaction,
            Err(error) => {
                self.delete_uncommitted_object(&key).await;
                return Err(database_error(error));
            }
        };
        let result = async {
            let current = self
                .exports
                .find_by_id_for_update_in_transaction(&transaction, export.id)
                .await?
                .ok_or_else(|| AppError::NotFound("导出任务不存在".into()))?;
            if current.status == export_job::Model::STATUS_SUCCEEDED {
                return if current.result_file_id == Some(file_id) {
                    Ok(false)
                } else {
                    Err(AppError::Conflict("导出任务结果文件标识冲突".into()))
                };
            }
            if current.status != export_job::Model::STATUS_RUNNING {
                return Err(AppError::Conflict("导出任务已不再允许运行".into()));
            }

            let file = self
                .files
                .insert_in_txn(&transaction, &export.tenant_id, file)
                .await?;
            let completed_at = self.background_jobs.database_utc_now(&transaction).await?;
            if !self
                .exports
                .mark_succeeded_in_transaction(
                    &transaction,
                    MarkExportJobSucceeded {
                        id: export.id,
                        file_id: file.id,
                        file_name,
                        content_type: file.content_type,
                        file_size: file.file_size,
                        expires_at: completed_at + self.export_retention,
                        completed_at,
                    },
                )
                .await?
            {
                return Err(AppError::Conflict("导出任务状态已变化".into()));
            }
            Ok(true)
        }
        .await;

        match result {
            Ok(_) => match crate::commit_current_audit(transaction).await {
                Ok(()) => Ok(()),
                Err(error) => {
                    self.compensate_uncommitted_object(export.id, &key).await;
                    Err(error)
                }
            },
            Err(error) => {
                let _ = transaction.rollback().await;
                self.compensate_uncommitted_object(export.id, &key).await;
                Err(error)
            }
        }
    }

    /// 事务失败后只删除未被成功状态引用的确定性对象；读取失败时宁可保留孤儿对象。
    async fn compensate_uncommitted_object(&self, export_id: i64, key: &str) {
        let Ok(transaction) = self.db.write().begin().await else {
            return;
        };
        let Ok(Some(current)) = self
            .exports
            .find_by_id_for_update_in_transaction(&transaction, export_id)
            .await
        else {
            let _ = transaction.rollback().await;
            return;
        };
        if should_delete_uncommitted_object(&current.status) {
            let _ = self.storage.delete(EXPORT_BUCKET, key).await;
        }
        let _ = transaction.rollback().await;
    }

    async fn delete_uncommitted_object(&self, key: &str) {
        if let Err(error) = self.storage.delete(EXPORT_BUCKET, key).await {
            tracing::warn!(%error, "清理未提交的导出对象失败，后续相同任务重试会覆盖确定性对象键");
        }
    }
}

async fn hash_file(path: &std::path::Path) -> AppResult<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| AppError::Internal(format!("打开导出临时文件失败: {error}")))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| AppError::Internal(format!("读取导出临时文件失败: {error}")))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}
