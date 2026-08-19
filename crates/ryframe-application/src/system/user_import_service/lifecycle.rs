impl UserImportService {
    async fn mark_running(&self, import_id: i64) -> AppResult<user_import_job::Model> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let mut import = UserImportRepository
            .lock_by_id_in_txn(&transaction, import_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        if import.is_terminal() {
            transaction.rollback().await.map_err(database_error)?;
            return Ok(import);
        }
        let now = UserImportRepository.database_utc_now(&transaction).await?;
        if import.cancel_requested {
            import.status = user_import_job::Model::STATUS_CANCELLED.to_owned();
            import.completed_at = Some(now);
        } else {
            import.status = user_import_job::Model::STATUS_RUNNING.to_owned();
            import.started_at.get_or_insert(now);
        }
        import.updated_at = now;
        let saved = UserImportRepository
            .save_in_txn(&transaction, import)
            .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(saved)
    }

    async fn set_total_rows(&self, import_id: i64, total: usize) -> AppResult<()> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let mut import = UserImportRepository
            .lock_by_id_in_txn(&transaction, import_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        let total = i32::try_from(total)
            .map_err(|_| AppError::Validation("用户导入行数超出数据库范围".into()))?;
        if import.processed_rows > total {
            return Err(AppError::Internal("用户导入进度超过源文件行数".into()));
        }
        let now = UserImportRepository.database_utc_now(&transaction).await?;
        import.total_rows = total;
        import.updated_at = now;
        UserImportRepository
            .save_in_txn(&transaction, import)
            .await?;
        transaction.commit().await.map_err(database_error)
    }

    async fn finalize_import(&self, import_id: i64) -> AppResult<user_import_job::Model> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let mut import = UserImportRepository
            .lock_by_id_in_txn(&transaction, import_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        if import.is_terminal() {
            transaction.rollback().await.map_err(database_error)?;
            return Ok(import);
        }
        if import.processed_rows != import.total_rows {
            return Err(AppError::Conflict("用户导入尚未处理完全部行".into()));
        }
        let now = UserImportRepository.database_utc_now(&transaction).await?;
        import.status = if import.failure_count > 0 || import.skipped_count > 0 {
            user_import_job::Model::STATUS_PARTIAL.to_owned()
        } else {
            user_import_job::Model::STATUS_SUCCEEDED.to_owned()
        };
        import.completed_at = Some(now);
        import.updated_at = now;
        import.last_error = None;
        let saved = UserImportRepository
            .save_in_txn(&transaction, import)
            .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(saved)
    }

    async fn mark_cancelled(&self, import_id: i64) -> AppResult<()> {
        self.mark_terminal(import_id, user_import_job::Model::STATUS_CANCELLED, None)
            .await
            .map(|_| ())
    }

    async fn mark_failed(&self, import_id: i64, error: &str) -> AppResult<user_import_job::Model> {
        self.mark_terminal(
            import_id,
            user_import_job::Model::STATUS_FAILED,
            Some(error),
        )
        .await
    }

    async fn mark_terminal(
        &self,
        import_id: i64,
        status: &str,
        error: Option<&str>,
    ) -> AppResult<user_import_job::Model> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let mut import = UserImportRepository
            .lock_by_id_in_txn(&transaction, import_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        let now = UserImportRepository.database_utc_now(&transaction).await?;
        import.status = status.to_owned();
        import.completed_at = Some(now);
        import.updated_at = now;
        import.last_error = error.map(truncate_error);
        let saved = UserImportRepository
            .save_in_txn(&transaction, import)
            .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(saved)
    }
}
