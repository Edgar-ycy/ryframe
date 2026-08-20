use super::*;

impl DataRetentionService {
    pub async fn execute_job(&self, job: &ClaimedBackgroundJob) -> AppResult<()> {
        let now = self.run_persistence.database_now().await?;
        let trigger_kind = job
            .payload
            .get("trigger_kind")
            .and_then(Value::as_str)
            .filter(|value| {
                matches!(
                    *value,
                    RetentionRunRecord::TRIGGER_MANUAL | RetentionRunRecord::TRIGGER_SCHEDULED
                )
            })
            .unwrap_or(RetentionRunRecord::TRIGGER_SCHEDULED);
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
            .unwrap_or(crate::next_id()?);
        self.run_persistence
            .insert_if_missing(new_run_record(
                proposed_id,
                job.id,
                trigger_kind,
                requested_by,
                now,
            ))
            .await?;
        let transaction = self.run_persistence.begin().await?;
        let run = transaction
            .lock_by_background_job(job.id)
            .await?
            .ok_or_else(|| AppError::NotFound("数据保留运行记录不存在".into()))?;
        // 后台任务可能在业务清理已经提交后丢失租约，并由管理员重新投递。完成态是可靠事实，
        // 通过行锁原子确认后直接返回，避免再次执行永久删除或重写原完成时间。
        let Some(mut run) = transaction.begin_run(run, now).await? else {
            transaction.commit().await?;
            return Ok(());
        };
        transaction.commit().await?;
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
        run = self.run_persistence.update(run).await?;

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
                run.updated_at = self.run_persistence.database_now().await?;
                run = self.run_persistence.update(run).await?;
            }
            Err(error) => {
                run.status = RetentionRunRecord::STATUS_FAILED.to_owned();
                run.error_summary = Some(safe_error_summary(&error));
                run.updated_at = self.run_persistence.database_now().await?;
                run.completed_at = Some(run.updated_at);
                let _ = self.run_persistence.update(run).await;
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
                run.updated_at = self.run_persistence.database_now().await?;
                run = self.run_persistence.update(run).await?;
            }
            Err(error) => {
                run.status = RetentionRunRecord::STATUS_FAILED.to_owned();
                run.error_summary = Some(safe_error_summary(&error));
                run.updated_at = self.run_persistence.database_now().await?;
                run.completed_at = Some(run.updated_at);
                let _ = self.run_persistence.update(run).await;
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
                    run.updated_at = self.run_persistence.database_now().await?;
                    run = self.run_persistence.update(run).await?;
                }
                Err(error) => {
                    run.status = RetentionRunRecord::STATUS_FAILED.to_owned();
                    run.error_summary = Some(safe_error_summary(&error));
                    run.updated_at = self.run_persistence.database_now().await?;
                    run.completed_at = Some(run.updated_at);
                    let _ = self.run_persistence.update(run).await;
                    return Err(error);
                }
            }
        }
        let completed_at = self.run_persistence.database_now().await?;
        run.status = if remaining.values().any(|count| *count > 0) {
            RetentionRunRecord::STATUS_PARTIAL
        } else {
            RetentionRunRecord::STATUS_SUCCEEDED
        }
        .to_owned();
        run.completed_at = Some(completed_at);
        run.updated_at = completed_at;
        self.run_persistence.update(run).await?;
        Ok(())
    }

    /// 在 Worker 领取后先建立运行记录，使外层超时或租约恢复也能同步公开状态。
    pub async fn prepare_job(&self, job: &ClaimedBackgroundJob) -> AppResult<()> {
        let now = self.run_persistence.database_now().await?;
        let trigger_kind = job
            .payload
            .get("trigger_kind")
            .and_then(Value::as_str)
            .filter(|value| {
                matches!(
                    *value,
                    RetentionRunRecord::TRIGGER_MANUAL | RetentionRunRecord::TRIGGER_SCHEDULED
                )
            })
            .unwrap_or(RetentionRunRecord::TRIGGER_SCHEDULED);
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
            .unwrap_or(crate::next_id()?);
        self.run_persistence
            .insert_if_missing(new_run_record(
                proposed_id,
                job.id,
                trigger_kind,
                requested_by,
                now,
            ))
            .await?;
        Ok(())
    }
}
