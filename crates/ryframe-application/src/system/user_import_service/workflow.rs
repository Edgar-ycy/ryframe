impl UserImportService {
    /// 执行后台导入；已提交批次通过持久化游标恢复，不会重复创建用户。
    pub async fn execute_background_job(&self, background_job_id: i64) -> AppResult<()> {
        let mut import = UserImportRepository
            .find_by_background_job(self.db.write(), background_job_id)
            .await?
            .ok_or_else(|| AppError::NotFound("后台任务没有关联用户导入记录".into()))?;
        if import.is_terminal() {
            return self.ensure_error_report_with_status(&import).await;
        }

        import = self.mark_running(import.id).await?;
        if import.status == user_import_job::Model::STATUS_CANCELLED {
            return Ok(());
        }
        let source = self
            .file_service
            .download_internal(&import.tenant_id, import.source_file_id, IMPORT_BUCKET)
            .await?;
        let source_sha256 = hex::encode(Sha256::digest(&source.data));
        if source_sha256 != import.source_sha256 {
            self.mark_failed(import.id, "导入源文件完整性校验失败")
                .await?;
            return Ok(());
        }
        let rows = tokio::task::spawn_blocking(move || {
            ExcelImporter::validate_headers_from_bytes(
                &source.data,
                None,
                UserImportData::excel_headers(),
            )?;
            ExcelImporter::read_rows_from_bytes::<UserImportData>(&source.data, None)
        })
        .await
        .map_err(|error| AppError::Internal(format!("用户导入解析任务异常结束: {error}")))??;

        if rows.len() > self.config.max_rows {
            self.mark_failed(
                import.id,
                &format!("导入行数超过 {} 条上限", self.config.max_rows),
            )
            .await?;
            return Ok(());
        }
        if rows.is_empty() {
            self.mark_failed(import.id, "导入文件未包含任何用户数据行")
                .await?;
            return Ok(());
        }
        self.set_total_rows(import.id, rows.len()).await?;
        self.process_rows(import.id, rows).await?;
        let finished = self.finalize_import(import.id).await?;
        self.ensure_error_report_with_status(&finished).await
    }

    async fn process_rows(
        &self,
        import_id: i64,
        rows: Vec<ExcelImportRow<UserImportData>>,
    ) -> AppResult<()> {
        let mut department_directory = None;
        loop {
            let current = user_import_job::Entity::find_by_id(import_id)
                .one(self.db.write())
                .await
                .map_err(database_error)?
                .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
            if current.cancel_requested {
                self.mark_cancelled(import_id).await?;
                return Ok(());
            }
            if current.is_terminal() {
                return Ok(());
            }
            let offset = usize::try_from(current.processed_rows)
                .map_err(|_| AppError::Internal("用户导入进度游标无效".into()))?;
            if offset >= rows.len() {
                return Ok(());
            }

            let authorization = match self
                .user_service
                .resolve_current_authorization(
                    &current.tenant_id,
                    current.requester_user_id,
                    USER_IMPORT_PERMISSION,
                )
                .await
            {
                Ok(authorization) => authorization,
                Err(error) if is_terminal_authorization_error(&error) => {
                    self.mark_failed(import_id, &error.to_string()).await?;
                    return Ok(());
                }
                Err(error) => return Err(error),
            };
            let authorization_epoch = authorization.tenant.authorization_epoch;
            if department_directory
                .as_ref()
                .is_none_or(|(epoch, _)| *epoch != authorization_epoch)
            {
                department_directory = Some((
                    authorization_epoch,
                    self.load_department_directory(&current.tenant_id).await?,
                ));
            }
            let directory = &department_directory
                .as_ref()
                .ok_or_else(|| AppError::Internal("部门路径目录未初始化".into()))?
                .1;
            let end = offset
                .saturating_add(self.config.batch_size)
                .min(rows.len());
            let prepared = self
                .prepare_batch(
                    &authorization.actor,
                    directory,
                    &rows[offset..end],
                    authorization.tenant.authorization_epoch,
                    authorization.user.authorization_version,
                )
                .await?;
            if self.commit_batch(import_id, offset, end, prepared).await?
                == CommitBatchOutcome::AuthorizationChanged
            {
                department_directory = None;
            }
        }
    }
}
