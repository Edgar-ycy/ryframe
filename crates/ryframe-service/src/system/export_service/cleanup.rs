use chrono::{DateTime, Utc};
use ryframe_db::entities::export_job;
use ryframe_kernel::AppResult;
use sea_orm::TransactionTrait;

use super::*;

impl ExportService {
    /// 清理过期导出结果。
    ///
    /// 整轮清理只读取一次数据库时间，并使用稳定主键游标排空所有已到期任务。
    /// 单个任务失败不会阻塞后续任务；完成本轮其余任务后返回首个错误，让后台任务重试失败项。
    pub async fn cleanup_expired(&self) -> AppResult<u64> {
        let now = self
            .background_jobs
            .database_utc_now(self.db.write())
            .await?;
        let mut cleaned = 0_u64;
        let mut failed = 0_u64;
        let mut first_error = None;
        let mut after_id = None;

        loop {
            let exports = self
                .exports
                .list_expired_succeeded_after_id(
                    self.db.write(),
                    now,
                    after_id,
                    EXPORT_CLEANUP_BATCH_SIZE,
                )
                .await?;
            if exports.is_empty() {
                break;
            }

            for export in exports {
                let export_id = export.id;
                after_id = Some(export_id);
                match self.cleanup_expired_export(export, now).await {
                    Ok(true) => cleaned += 1,
                    Ok(false) => {}
                    Err(error) => {
                        failed += 1;
                        tracing::warn!(
                            export_id,
                            %error,
                            "单个过期导出结果清理失败，将继续处理后续任务"
                        );
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
            }
        }

        if let Some(error) = first_error {
            tracing::warn!(cleaned, failed, "过期导出结果已完成部分清理，将重试失败项");
            Err(error)
        } else {
            Ok(cleaned)
        }
    }

    /// 清理一个已在固定时间快照下到期的导出结果。
    async fn cleanup_expired_export(
        &self,
        export: export_job::Model,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let Some(file_id) = export.result_file_id else {
            return self.mark_export_expired(export.id, now).await;
        };
        let Some(file) = self
            .files
            .find_by_id(self.db.write(), &export.tenant_id, file_id)
            .await?
        else {
            return self.mark_export_expired(export.id, now).await;
        };
        self.storage
            .delete(&file.bucket, &file.storage_path)
            .await
            .map_err(storage_error)?;
        self.delete_export_file_and_mark_expired(&export.tenant_id, export.id, file.id, now)
            .await
    }

    /// 在后台清理短事务内将无文件元数据的导出任务改为过期。
    async fn mark_export_expired(&self, export_id: i64, now: DateTime<Utc>) -> AppResult<bool> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let marked = self
            .exports
            .mark_expired(&transaction, export_id, now)
            .await?;
        crate::commit_current_audit(transaction).await?;
        Ok(marked)
    }

    /// 原子软删除导出文件元数据并将任务改为过期。
    ///
    /// 对象存储删除先于该事务执行；若事务失败，下一轮会再次执行幂等删除，
    /// 不会留下已过期任务仍引用可见文件元数据的中间状态。
    async fn delete_export_file_and_mark_expired(
        &self,
        tenant_id: &str,
        export_id: i64,
        file_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let result = async {
            let current = self
                .exports
                .find_by_id_for_update_in_transaction(&transaction, export_id)
                .await?;
            let still_expired = current.is_some_and(|current| {
                current.tenant_id == tenant_id
                    && current.status == export_job::Model::STATUS_SUCCEEDED
                    && current.result_file_id == Some(file_id)
                    && current
                        .expires_at
                        .is_some_and(|expires_at| expires_at <= now)
            });
            if !still_expired {
                return Ok(false);
            }
            self.files
                .delete_in_txn(&transaction, tenant_id, file_id)
                .await?;
            self.exports
                .mark_expired(&transaction, export_id, now)
                .await
        }
        .await;
        match result {
            Ok(true) => {
                crate::commit_current_audit(transaction).await?;
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
}
