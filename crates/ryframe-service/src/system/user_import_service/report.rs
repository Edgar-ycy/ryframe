impl UserImportService {
    async fn ensure_error_report(&self, import: &user_import_job::Model) -> AppResult<()> {
        if import.error_report_file_id.is_some()
            || import.failure_count.saturating_add(import.skipped_count) == 0
        {
            return Ok(());
        }
        let rows = UserImportRepository
            .all_row_results(self.db.write(), &import.tenant_id, import.id)
            .await?;
        if rows.is_empty() {
            return Ok(());
        }
        let report_rows = rows
            .into_iter()
            .map(|row| UserImportReportRow {
                row_number: row.row_number,
                username: row.username_snapshot,
                outcome: row.outcome,
                code: row.code,
                message: row.message,
            })
            .collect::<Vec<_>>();
        let bytes = tokio::task::spawn_blocking(move || {
            ExcelExporter::export_to_bytes(
                &report_rows,
                "导入结果",
                UserImportReportRow::excel_headers(),
            )
        })
        .await
        .map_err(|error| AppError::Internal(format!("用户导入报告生成任务异常结束: {error}")))??;
        let report_sha256 = hex::encode(Sha256::digest(&bytes));
        let mut config = self.upload_config();
        config.max_file_size = config
            .max_file_size
            .max(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        let uploaded = self
            .file_service
            .upload_internal(
                &import.tenant_id,
                "system:user-import",
                UploadCommand {
                    original_name: "user_import_report.xlsx".to_owned(),
                    data: bytes,
                    config: &config,
                    bucket: IMPORT_BUCKET,
                    compress: false,
                },
            )
            .await?;
        let file_id = uploaded
            .file_id
            .parse::<i64>()
            .map_err(|_| AppError::Internal("用户导入报告文件标识无效".into()))?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        // 报告文件可能因内容相同而被多个任务复用。与导入创建及历史清理统一使用
        // tenant -> file -> import 的锁序，并在写引用前重新确认对象仍可用。
        TenantRepository
            .lock_tenant_in_txn(&transaction, &import.tenant_id)
            .await?;
        let report_file = FileRepository
            .find_by_id_any_status_for_update(&transaction, &import.tenant_id, file_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入报告文件已被回收".into()))?;
        if report_file.bucket != IMPORT_BUCKET
            || report_file.upload_status
                != ryframe_db::entities::sys_file::Model::UPLOAD_STATUS_READY
            || report_file.file_sha256 != report_sha256
        {
            return Err(AppError::Conflict("用户导入报告文件状态已变化".into()));
        }
        let mut current = UserImportRepository
            .lock_by_id_in_txn(&transaction, import.id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        let now = UserImportRepository.database_utc_now(&transaction).await?;
        if current.error_report_file_id.is_none() && current.is_terminal() {
            current.error_report_file_id = Some(file_id);
            current.updated_at = now;
            current.last_error = None;
            UserImportRepository
                .save_in_txn(&transaction, current)
                .await?;
            transaction.commit().await.map_err(database_error)?;
        } else {
            // 并发执行已经关联报告，或人工重投已使任务离开终态时，本次上传可能成为
            // 无引用对象。只为确实没有任何引用的文件建立可恢复墓碑。
            let marked = FileRepository
                .mark_import_orphan_for_cleanup_in_txn(
                    &transaction,
                    &import.tenant_id,
                    file_id,
                    now,
                    now + chrono::Duration::minutes(IMPORT_ORPHAN_CLEANUP_GRACE_MINUTES),
                )
                .await?;
            if marked {
                transaction.commit().await.map_err(database_error)?;
            } else {
                transaction.rollback().await.map_err(database_error)?;
            }
        }
        Ok(())
    }

    /// 生成异常报告，并只在明确的报告阶段记录错误，避免把租约或队列错误写入已完成导入。
    async fn ensure_error_report_with_status(
        &self,
        import: &user_import_job::Model,
    ) -> AppResult<()> {
        match self.ensure_error_report(import).await {
            Ok(()) => Ok(()),
            Err(error) => {
                if let Err(record_error) = self
                    .record_error_report_failure(import.id, &error.to_string())
                    .await
                {
                    tracing::error!(
                        import_id = import.id,
                        %record_error,
                        "记录用户导入异常报告失败状态时发生错误"
                    );
                }
                Err(error)
            }
        }
    }

    async fn record_error_report_failure(&self, import_id: i64, error: &str) -> AppResult<()> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let Some(mut import) = UserImportRepository
            .lock_by_id_in_txn(&transaction, import_id)
            .await?
        else {
            transaction.rollback().await.map_err(database_error)?;
            return Ok(());
        };
        if import.status != user_import_job::Model::STATUS_PARTIAL {
            transaction.rollback().await.map_err(database_error)?;
            return Ok(());
        }
        let now = UserImportRepository.database_utc_now(&transaction).await?;
        import.last_error = Some(truncate_error(error));
        import.updated_at = now;
        UserImportRepository
            .save_in_txn(&transaction, import)
            .await?;
        transaction.commit().await.map_err(database_error)
    }
}
