use super::*;

impl DataRetentionService {
    pub async fn execute_job(&self, job: &background_job::Model) -> AppResult<()> {
        let now = self.repository.database_utc_now(self.db.write()).await?;
        let trigger_kind = job
            .payload
            .get("trigger_kind")
            .and_then(Value::as_str)
            .filter(|value| {
                matches!(
                    *value,
                    data_retention_run::Model::TRIGGER_MANUAL
                        | data_retention_run::Model::TRIGGER_SCHEDULED
                )
            })
            .unwrap_or(data_retention_run::Model::TRIGGER_SCHEDULED);
        let requested_by = job
            .payload
            .get("requested_by")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<i64>().ok());
        let proposed_id = job
            .payload
            .get("run_id")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(ryframe_adapters::snowflake::try_next_snowflake_id()?);
        self.repository
            .insert_run_if_missing(
                self.db.write(),
                new_run_model(proposed_id, job.id, trigger_kind, requested_by, now),
            )
            .await?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let run = self
            .repository
            .lock_run_by_background_job_in_txn(&transaction, job.id)
            .await?
            .ok_or_else(|| AppError::NotFound("数据保留运行记录不存在".into()))?;
        // 后台任务可能在业务清理已经提交后丢失租约，并由管理员重新投递。完成态是可靠事实，
        // 通过行锁原子确认后直接返回，避免再次执行永久删除或重写原完成时间。
        let Some(mut run) = self
            .repository
            .begin_run_in_txn(&transaction, run, now)
            .await?
        else {
            transaction.commit().await.map_err(database_error)?;
            return Ok(());
        };
        transaction.commit().await.map_err(database_error)?;
        let overview = self.overview_at(now);
        run.policy_snapshot = serde_json::to_value(&overview).map_err(json_error)?;
        let cutoffs = self.cutoffs(now);
        let mut eligible = self
            .repository
            .preview(self.db.write(), &cutoffs, Some(run.id))
            .await?;
        eligible.insert(
            "user_import_artifacts".to_owned(),
            UserImportRepository
                .count_expired_artifacts(self.db.write(), self.import_artifact_cutoff(now))
                .await?,
        );
        eligible.extend(self.preview_tenant_config_artifacts(now).await?);
        run.eligible_counts = serde_json::to_value(&eligible).map_err(json_error)?;
        run = self.repository.update_run(self.db.write(), run).await?;

        let mut deleted = json_counts(&run.deleted_counts);
        let mut remaining = BTreeMap::new();
        match self.cleanup_import_artifacts(now).await {
            Ok(result) => {
                *deleted
                    .entry("user_import_artifacts".to_owned())
                    .or_default() += result.deleted;
                remaining.insert("user_import_artifacts".to_owned(), result.remaining);
                run.deleted_counts = serde_json::to_value(&deleted).map_err(json_error)?;
                run.remaining_counts = serde_json::to_value(&remaining).map_err(json_error)?;
                run.updated_at = self.repository.database_utc_now(self.db.write()).await?;
                run = self.repository.update_run(self.db.write(), run).await?;
            }
            Err(error) => {
                run.status = data_retention_run::Model::STATUS_FAILED.to_owned();
                run.error_summary = Some(safe_error_summary(&error));
                run.updated_at = self.repository.database_utc_now(self.db.write()).await?;
                run.completed_at = Some(run.updated_at);
                let _ = self.repository.update_run(self.db.write(), run).await;
                return Err(error);
            }
        }
        match self.cleanup_tenant_config_artifacts(now).await {
            Ok(counts) => {
                for (resource, count) in counts {
                    *deleted.entry(resource.clone()).or_default() += count.deleted;
                    remaining.insert(resource, count.remaining);
                }
                run.deleted_counts = serde_json::to_value(&deleted).map_err(json_error)?;
                run.remaining_counts = serde_json::to_value(&remaining).map_err(json_error)?;
                run.updated_at = self.repository.database_utc_now(self.db.write()).await?;
                run = self.repository.update_run(self.db.write(), run).await?;
            }
            Err(error) => {
                run.status = data_retention_run::Model::STATUS_FAILED.to_owned();
                run.error_summary = Some(safe_error_summary(&error));
                run.updated_at = self.repository.database_utc_now(self.db.write()).await?;
                run.completed_at = Some(run.updated_at);
                let _ = self.repository.update_run(self.db.write(), run).await;
                return Err(error);
            }
        }
        for cutoff in cutoffs {
            match self
                .repository
                .cleanup_resource(
                    self.db.write(),
                    cutoff,
                    self.config.cleanup_batch_size,
                    self.config.max_rows_per_resource_per_run,
                    Some(run.id),
                )
                .await
            {
                Ok(result) => {
                    *deleted.entry(cutoff.resource.key().to_owned()).or_default() += result.deleted;
                    remaining.insert(cutoff.resource.key().to_owned(), result.remaining);
                    run.deleted_counts = serde_json::to_value(&deleted).map_err(json_error)?;
                    run.remaining_counts = serde_json::to_value(&remaining).map_err(json_error)?;
                    run.updated_at = self.repository.database_utc_now(self.db.write()).await?;
                    run = self.repository.update_run(self.db.write(), run).await?;
                }
                Err(error) => {
                    run.status = data_retention_run::Model::STATUS_FAILED.to_owned();
                    run.error_summary = Some(safe_error_summary(&error));
                    run.updated_at = self.repository.database_utc_now(self.db.write()).await?;
                    run.completed_at = Some(run.updated_at);
                    let _ = self.repository.update_run(self.db.write(), run).await;
                    return Err(error);
                }
            }
        }
        let completed_at = self.repository.database_utc_now(self.db.write()).await?;
        run.status = if remaining.values().any(|count| *count > 0) {
            data_retention_run::Model::STATUS_PARTIAL
        } else {
            data_retention_run::Model::STATUS_SUCCEEDED
        }
        .to_owned();
        run.completed_at = Some(completed_at);
        run.updated_at = completed_at;
        self.repository.update_run(self.db.write(), run).await?;
        Ok(())
    }

    /// 在 Worker 领取后先建立运行记录，使外层超时或租约恢复也能同步公开状态。
    pub async fn prepare_job(&self, job: &background_job::Model) -> AppResult<()> {
        let now = self.repository.database_utc_now(self.db.write()).await?;
        let trigger_kind = job
            .payload
            .get("trigger_kind")
            .and_then(Value::as_str)
            .filter(|value| {
                matches!(
                    *value,
                    data_retention_run::Model::TRIGGER_MANUAL
                        | data_retention_run::Model::TRIGGER_SCHEDULED
                )
            })
            .unwrap_or(data_retention_run::Model::TRIGGER_SCHEDULED);
        let requested_by = job
            .payload
            .get("requested_by")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<i64>().ok());
        let proposed_id = job
            .payload
            .get("run_id")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(ryframe_adapters::snowflake::try_next_snowflake_id()?);
        self.repository
            .insert_run_if_missing(
                self.db.write(),
                new_run_model(proposed_id, job.id, trigger_kind, requested_by, now),
            )
            .await?;
        Ok(())
    }
}
