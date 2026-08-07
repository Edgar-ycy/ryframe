use chrono::{DateTime, Utc};
use ryframe_db::{
    MarkExportJobSucceeded,
    entities::{export_job, sys_file},
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::TransactionTrait;
use sha2::Digest;

use super::*;

impl ExportService {
    pub(super) async fn persist_export_file(
        &self,
        export: export_job::Model,
        actor: ActorContext,
        bytes: Vec<u8>,
        resource: &str,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let (file_name, key) = export_file_location(&export.tenant_id, resource, export.id);
        let file_id = deterministic_export_file_id(export.id);
        let content_type =
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_owned();
        let file_size = i64::try_from(bytes.len())
            .map_err(|_| AppError::PayloadTooLarge("导出文件超过数据库大小范围".into()))?;
        let file_sha256 = hex::encode(sha2::Sha256::digest(&bytes));
        self.storage
            .ensure_bucket(EXPORT_BUCKET)
            .await
            .map_err(storage_error)?;
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
        let transaction = self.db.write().begin().await.map_err(database_error)?;
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

            self.storage
                .put(EXPORT_BUCKET, &key, &bytes, &content_type)
                .await
                .map_err(storage_error)?;
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
                        request_params: export.request_params,
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
}
