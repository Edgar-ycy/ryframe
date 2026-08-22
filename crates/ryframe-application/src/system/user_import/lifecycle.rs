impl UserImportService {
    async fn mark_running(&self, import_id: i64) -> AppResult<UserImportJobRecord> {
        let transaction = self.persistence.begin().await?;
        let mut import = transaction
            .lock(import_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        if import.is_terminal() {
            transaction.rollback().await?;
            return Ok(import);
        }
        let now = transaction.database_now().await?;
        if import.cancel_requested {
            import.status = UserImportJobRecord::STATUS_CANCELLED.to_owned();
            import.completed_at = Some(now);
        } else {
            import.status = UserImportJobRecord::STATUS_RUNNING.to_owned();
            import.started_at.get_or_insert(now);
        }
        import.updated_at = now;
        let saved = transaction.save(import).await?;
        transaction.commit().await?;
        Ok(saved)
    }

    async fn set_total_rows(&self, import_id: i64, total: usize) -> AppResult<()> {
        let transaction = self.persistence.begin().await?;
        let mut import = transaction
            .lock(import_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        let total = i32::try_from(total)
            .map_err(|_| AppError::Validation("用户导入行数超出数据库范围".into()))?;
        if import.processed_rows > total {
            return Err(AppError::Internal("用户导入进度超过源文件行数".into()));
        }
        let now = transaction.database_now().await?;
        import.total_rows = total;
        import.updated_at = now;
        transaction.save(import).await?;
        transaction.commit().await
    }

    async fn finalize_import(&self, import_id: i64) -> AppResult<UserImportJobRecord> {
        let transaction = self.persistence.begin().await?;
        let mut import = transaction
            .lock(import_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        if import.is_terminal() {
            transaction.rollback().await?;
            return Ok(import);
        }
        if import.processed_rows != import.total_rows {
            return Err(AppError::Conflict("用户导入尚未处理完全部行".into()));
        }
        let now = transaction.database_now().await?;
        import.status = if import.failure_count > 0 || import.skipped_count > 0 {
            UserImportJobRecord::STATUS_PARTIAL.to_owned()
        } else {
            UserImportJobRecord::STATUS_SUCCEEDED.to_owned()
        };
        import.completed_at = Some(now);
        import.updated_at = now;
        import.last_error = None;
        let saved = transaction.save(import).await?;
        transaction.commit().await?;
        Ok(saved)
    }

    async fn mark_cancelled(&self, import_id: i64) -> AppResult<()> {
        self.mark_terminal(import_id, UserImportJobRecord::STATUS_CANCELLED, None)
            .await
            .map(|_| ())
    }

    async fn mark_failed(&self, import_id: i64, error: &str) -> AppResult<UserImportJobRecord> {
        self.mark_terminal(
            import_id,
            UserImportJobRecord::STATUS_FAILED,
            Some(error),
        )
        .await
    }

    async fn mark_terminal(
        &self,
        import_id: i64,
        status: &str,
        error: Option<&str>,
    ) -> AppResult<UserImportJobRecord> {
        let transaction = self.persistence.begin().await?;
        let mut import = transaction
            .lock(import_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        let now = transaction.database_now().await?;
        import.status = status.to_owned();
        import.completed_at = Some(now);
        import.updated_at = now;
        import.last_error = error.map(truncate_error);
        let saved = transaction.save(import).await?;
        transaction.commit().await?;
        Ok(saved)
    }
}
