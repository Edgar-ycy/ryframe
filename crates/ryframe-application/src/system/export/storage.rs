use super::*;
use ryframe_kernel::{ActorContext, AppError, AppResult};

use crate::ports::export::{CompleteExportArtifact, ExportArtifactFileDraft, ExportArtifactState};

impl ExportService {
    pub(super) async fn persist_export_file(
        &self,
        export_id: i64,
        tenant_id: &str,
        actor: ActorContext,
        artifact: Box<dyn crate::ports::spreadsheet::SpreadsheetArtifact>,
        resource: &str,
    ) -> AppResult<()> {
        let (file_name, key) = export_file_location(tenant_id, resource, export_id);
        let file_id = deterministic_export_file_id(export_id);
        let content_type =
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_owned();
        let file_size = i64::try_from(artifact.size())
            .map_err(|_| AppError::PayloadTooLarge("导出文件超过数据库大小范围".into()))?;
        let file_sha256 = artifact.sha256().to_owned();
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
        let transaction = match self.artifact_persistence.begin().await {
            Ok(transaction) => transaction,
            Err(error) => {
                self.delete_uncommitted_object(&key).await;
                return Err(error);
            }
        };
        let result = async {
            let current = transaction
                .lock_export(export_id)
                .await?
                .ok_or_else(|| AppError::NotFound("导出任务不存在".into()))?;
            if !artifact_write_required(&current, file_id)? {
                return Ok(false);
            }

            let now = transaction.database_now().await?;
            let file = transaction
                .insert_ready_file(
                    tenant_id,
                    ExportArtifactFileDraft {
                        id: file_id,
                        file_name,
                        storage_path: key.clone(),
                        bucket: EXPORT_BUCKET.into(),
                        file_url: format!("{EXPORT_BUCKET}/{key}"),
                        file_size,
                        content_type,
                        sha256: file_sha256,
                        uploaded_by: actor.username,
                        created_at: now,
                    },
                )
                .await?;
            let completed_at = transaction.database_now().await?;
            if !transaction
                .mark_succeeded(CompleteExportArtifact {
                    export_id,
                    file_id: file.id,
                    file_name: file.file_name,
                    content_type: file.content_type,
                    file_size: file.file_size,
                    expires_at: completed_at + self.export_retention,
                    completed_at,
                })
                .await?
            {
                return Err(AppError::Conflict("导出任务状态已变化".into()));
            }
            Ok(true)
        }
        .await;

        match result {
            Ok(_) => match transaction.commit().await {
                Ok(()) => Ok(()),
                Err(error) => {
                    self.compensate_uncommitted_object(export_id, &key).await;
                    Err(error)
                }
            },
            Err(error) => {
                let _ = transaction.rollback().await;
                self.compensate_uncommitted_object(export_id, &key).await;
                Err(error)
            }
        }
    }

    /// 事务失败后只删除未被成功状态引用的确定性对象；读取失败时宁可保留孤儿对象。
    async fn compensate_uncommitted_object(&self, export_id: i64, key: &str) {
        let Ok(transaction) = self.artifact_persistence.begin().await else {
            return;
        };
        let Ok(Some(current)) = transaction.lock_export(export_id).await else {
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

pub fn artifact_write_required(current: &ExportArtifactState, file_id: i64) -> AppResult<bool> {
    if current.status == EXPORT_STATUS_SUCCEEDED {
        return if current.result_file_id == Some(file_id) {
            Ok(false)
        } else {
            Err(AppError::Conflict("导出任务结果文件标识冲突".into()))
        };
    }
    if current.status != EXPORT_STATUS_RUNNING {
        return Err(AppError::Conflict("导出任务已不再允许运行".into()));
    }
    Ok(true)
}
