use ryframe_kernel::AppResult;

use super::*;

impl ExportService {
    /// 重试用户删除墓碑并清理过期导出结果。
    ///
    /// 整轮只读取一次数据库时间，单项失败不会阻塞后续项；最终返回首个错误，
    /// 使后台任务保留告警并按既有退避策略重试尚未清理的墓碑。
    pub async fn cleanup_expired(&self) -> AppResult<u64> {
        let now = self.cleanup_persistence.database_now().await?;
        let mut cleaned = 0_u64;
        let mut failed = 0_u64;
        let mut first_error = None;

        let mut after_id = None;
        loop {
            let exports = self
                .cleanup_persistence
                .list_delete_pending(after_id, EXPORT_CLEANUP_BATCH_SIZE)
                .await?;
            if exports.is_empty() {
                break;
            }
            for export in exports {
                let export_id = export.id;
                after_id = Some(export_id);
                match self.purge.purge_deleted_export(export).await {
                    Ok(true) => cleaned += 1,
                    Ok(false) => {}
                    Err(error) => {
                        failed += 1;
                        tracing::warn!(
                            export_id,
                            %error,
                            "导出删除墓碑清理失败，将继续处理后续任务"
                        );
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
            }
        }

        after_id = None;
        loop {
            let exports = self
                .cleanup_persistence
                .list_expired(now, after_id, EXPORT_CLEANUP_BATCH_SIZE)
                .await?;
            if exports.is_empty() {
                break;
            }
            for export in exports {
                let export_id = export.id;
                after_id = Some(export_id);
                match self.purge.purge_expired_export(export, now).await {
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
            tracing::warn!(cleaned, failed, "导出清理已完成部分任务，将重试失败项");
            Err(error)
        } else {
            Ok(cleaned)
        }
    }
}
