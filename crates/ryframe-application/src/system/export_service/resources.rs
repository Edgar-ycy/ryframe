use std::time::{Duration as StdDuration, Instant};

use crate::{
    LoginInfoFilter, SPREADSHEET_MAX_DATA_ROWS, SpreadsheetArtifact, SpreadsheetBatchProgress,
    SpreadsheetWriter,
};
use ryframe_db::{
    ExportStartDisposition, OperLogFilter, UserFilter,
    entities::{background_job, export_job},
};
use ryframe_kernel::{ActorContext, AppError, AppResult, ExportCursorWindow};
use sea_orm::TransactionTrait;

use super::*;

struct ExportExecution<'a> {
    background_job_id: i64,
    lease_owner: &'a str,
    started_at: Instant,
    matched_rows: u64,
}

struct AppendedBatch {
    last_id: i64,
    progress: SpreadsheetBatchProgress,
}

impl ExportService {
    /// 执行一个已领取的后台导出任务。
    pub async fn execute_background_job(
        &self,
        background_job: &background_job::Model,
        payload: &ExportJobPayload,
    ) -> AppResult<()> {
        payload.validate()?;
        let lease_owner = background_job
            .lease_owner
            .as_deref()
            .ok_or_else(|| AppError::ServiceUnavailable("导出后台任务缺少有效租约".into()))?;
        let now = self
            .background_jobs
            .database_utc_now(self.db.write())
            .await?;
        let mut export = self
            .exports
            .find_by_background_job_id(self.db.write(), background_job.id)
            .await?
            .ok_or_else(|| AppError::NotFound("后台任务未关联导出请求".into()))?;
        if export.resource != payload.resource() {
            return Err(AppError::Validation(
                "导出后台任务资源与公开任务资源不一致".into(),
            ));
        }
        if export.status == export_job::Model::STATUS_CANCELLED {
            return Ok(());
        }

        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let disposition = self
            .exports
            .try_mark_running_in_transaction(
                &transaction,
                export.id,
                &export.tenant_id,
                EXPORT_MAX_RUNNING_PER_TENANT,
                now,
            )
            .await?;
        crate::commit_current_audit(transaction).await?;
        match disposition {
            ExportStartDisposition::Started => {}
            ExportStartDisposition::AlreadyRunning => {
                return Err(AppError::ServiceUnavailable(
                    "同一导出任务已经被 Worker 执行".into(),
                ));
            }
            ExportStartDisposition::ConcurrencyLimited => {
                return Err(AppError::ServiceUnavailable(
                    "当前租户已有两个导出任务正在执行，请稍后重试".into(),
                ));
            }
            ExportStartDisposition::NotRunnable => return Ok(()),
        }

        let request: StoredExportRequest =
            serde_json::from_value(std::mem::take(&mut export.request_params))
                .map_err(|error| AppError::Validation(format!("导出请求快照无效: {error}")))?;
        request.validate(&export.resource)?;
        request.validate_persisted_snapshot(&export)?;
        let (actor, authorization_fingerprint) = self
            .users
            .resolve_current_export_authorization(
                &export.tenant_id,
                export.requester_id,
                &export.permission_code,
            )
            .await?;
        ensure_download_authorization_matches(
            &request.authorization_fingerprint,
            &authorization_fingerprint,
        )?;
        let execution = ExportExecution {
            background_job_id: background_job.id,
            lease_owner,
            started_at: Instant::now(),
            matched_rows: request.matched_rows,
        };
        self.stream_export(
            export,
            actor,
            &request.selection,
            request.upper_id,
            &execution,
        )
        .await
    }

    async fn stream_export(
        &self,
        export: export_job::Model,
        actor: ActorContext,
        selection: &ExportSelection,
        upper_id: i64,
        execution: &ExportExecution<'_>,
    ) -> AppResult<()> {
        let (sheet_name, headers) = export_layout(selection);
        let mut writer = self.spreadsheets.create(sheet_name, headers)?;
        let mut after_id = None;

        loop {
            if !self
                .ensure_execution_active(
                    &export,
                    execution,
                    writer.data_rows(),
                    writer.input_bytes(),
                )
                .await?
            {
                return Ok(());
            }
            let window = ExportCursorWindow::new(after_id, upper_id, EXPORT_BATCH_SIZE);
            let Some(batch) = self
                .append_export_batch(writer.as_mut(), &actor, selection, window)
                .await?
            else {
                break;
            };
            validate_batch_progress(&batch, execution.matched_rows, self.export_max_rows)?;
            if !self
                .ensure_execution_active(
                    &export,
                    execution,
                    batch.progress.total_rows,
                    batch.progress.total_input_bytes,
                )
                .await?
            {
                return Ok(());
            }
            let exported_rows = i64::try_from(batch.progress.total_rows)
                .map_err(|_| AppError::PayloadTooLarge("导出进度超过数据库范围".into()))?;
            let progress_at = self
                .background_jobs
                .database_utc_now(self.db.write())
                .await?;
            if !self
                .exports
                .update_exported_rows(self.db.write(), export.id, exported_rows, progress_at)
                .await?
            {
                return Err(AppError::Conflict("导出任务状态已变化".into()));
            }
            after_id = Some(batch.last_id);
            if batch.progress.batch_rows < EXPORT_BATCH_SIZE {
                break;
            }
        }

        if !self
            .ensure_execution_active(&export, execution, writer.data_rows(), writer.input_bytes())
            .await?
        {
            return Ok(());
        }
        let artifact = finish_writer_within_deadline(writer, execution).await?;
        validate_artifact_limits(
            artifact.as_ref(),
            execution.matched_rows,
            self.export_max_rows,
        )?;
        if !self
            .ensure_execution_active(
                &export,
                execution,
                artifact.data_rows(),
                artifact.input_bytes(),
            )
            .await?
        {
            return Ok(());
        }
        self.persist_export_file(export, actor, artifact, selection.resource())
            .await
    }

    async fn ensure_execution_active(
        &self,
        export: &export_job::Model,
        execution: &ExportExecution<'_>,
        rows: u64,
        input_bytes: u64,
    ) -> AppResult<bool> {
        validate_runtime_limits(execution, rows, input_bytes, self.export_max_rows)?;
        let now = self
            .background_jobs
            .database_utc_now(self.db.write())
            .await?;
        let background = self
            .background_jobs
            .find_by_id(self.db.write(), execution.background_job_id)
            .await?
            .ok_or_else(|| AppError::ServiceUnavailable("导出后台任务已不存在".into()))?;
        if background.status != background_job::Model::STATUS_RUNNING
            || background.lease_owner.as_deref() != Some(execution.lease_owner)
            || background
                .lease_until
                .is_none_or(|lease_until| lease_until <= now)
        {
            return Err(AppError::ServiceUnavailable(
                "导出后台任务租约已失效".into(),
            ));
        }
        let current = self
            .exports
            .find_by_id(self.db.write(), export.id)
            .await?
            .ok_or_else(|| AppError::NotFound("导出任务不存在".into()))?;
        if current.status == export_job::Model::STATUS_CANCELLED {
            return Ok(false);
        }
        if current.status != export_job::Model::STATUS_RUNNING
            || current.delete_pending_at.is_some()
        {
            return Err(AppError::Conflict("导出任务已不再允许运行".into()));
        }
        Ok(true)
    }

    async fn append_export_batch(
        &self,
        writer: &mut dyn SpreadsheetWriter,
        actor: &ActorContext,
        selection: &ExportSelection,
        window: ExportCursorWindow,
    ) -> AppResult<Option<AppendedBatch>> {
        match selection {
            ExportSelection::Users(filters) => {
                let batch = self
                    .users
                    .find_export_batch(
                        actor,
                        UserFilter {
                            username: filters.username(),
                            phone: filters.phone(),
                            status: filters.status(),
                            dept_id: filters.dept_id(),
                        },
                        window,
                    )
                    .await?;
                let Some(last_id) = last_batch_id(&batch, window, |item| &item.id)? else {
                    return Ok(None);
                };
                let progress = append_rows(
                    writer,
                    batch.into_iter().map(|user| {
                        serde_json::json!({
                            "user_id": user.id,
                            "username": user.username,
                            "nickname": user.nickname,
                            "email": user.email,
                            "phone": user.phone,
                            "dept_name": user.dept_name,
                            "status": user.status,
                            "remark": user.remark,
                            "created_at": user.created_at.to_rfc3339(),
                        })
                    }),
                )?;
                Ok(Some(AppendedBatch { last_id, progress }))
            }
            ExportSelection::Roles(filters) => {
                let batch = self
                    .roles
                    .find_export_batch(
                        actor,
                        filters.name(),
                        filters.code(),
                        filters.status(),
                        window,
                    )
                    .await?;
                let Some(last_id) = last_batch_id(&batch, window, |item| &item.id)? else {
                    return Ok(None);
                };
                let progress = append_rows(
                    writer,
                    batch.into_iter().map(|item| {
                        serde_json::json!({
                            "role_id": item.id, "role_name": item.name, "role_code": item.code,
                            "data_scope": item.data_scope, "status": item.status, "sort": item.sort,
                            "remark": item.remark, "created_at": item.created_at.to_rfc3339(),
                        })
                    }),
                )?;
                Ok(Some(AppendedBatch { last_id, progress }))
            }
            ExportSelection::Posts(filters) => {
                let batch = self
                    .posts
                    .find_export_batch(
                        actor,
                        filters.name(),
                        filters.code(),
                        filters.status(),
                        window,
                    )
                    .await?;
                let Some(last_id) = last_batch_id(&batch, window, |item| &item.id)? else {
                    return Ok(None);
                };
                let progress = append_rows(
                    writer,
                    batch.into_iter().map(|item| {
                        serde_json::json!({
                            "post_id": item.id, "name": item.name, "code": item.code,
                            "sort": item.sort, "status": item.status, "remark": item.remark,
                            "created_at": item.created_at.to_rfc3339(),
                        })
                    }),
                )?;
                Ok(Some(AppendedBatch { last_id, progress }))
            }
            ExportSelection::Configs(filters) => {
                let batch = self
                    .configs
                    .find_export_batch(actor, filters.name(), filters.key(), window)
                    .await?;
                let Some(last_id) = last_batch_id(&batch, window, |item| &item.id)? else {
                    return Ok(None);
                };
                let progress = append_rows(
                    writer,
                    batch.into_iter().map(|item| {
                        serde_json::json!({
                            "name": item.name, "key": item.key, "value": item.value,
                            "remark": item.remark, "created_at": item.created_at.to_rfc3339(),
                        })
                    }),
                )?;
                Ok(Some(AppendedBatch { last_id, progress }))
            }
            ExportSelection::DictTypes(filters) => {
                let batch = self
                    .dicts
                    .find_type_export_batch(
                        actor,
                        filters.name(),
                        filters.code(),
                        filters.status(),
                        window,
                    )
                    .await?;
                let Some(last_id) = last_batch_id(&batch, window, |item| &item.id)? else {
                    return Ok(None);
                };
                let progress = append_rows(
                    writer,
                    batch.into_iter().map(|item| {
                        serde_json::json!({
                            "name": item.name, "code": item.code, "status": item.status,
                            "remark": item.remark, "created_at": item.created_at.to_rfc3339(),
                        })
                    }),
                )?;
                Ok(Some(AppendedBatch { last_id, progress }))
            }
            ExportSelection::OperLogs(filters) => {
                let batch = self
                    .oper_logs
                    .find_export_batch(
                        actor,
                        OperLogFilter {
                            oper_name: filters.oper_name(),
                            status: filters.status(),
                            begin_time: filters.begin_time(),
                            end_time: filters.end_time(),
                        },
                        window,
                    )
                    .await?;
                let Some(last_id) = last_batch_id(&batch, window, |item| &item.id)? else {
                    return Ok(None);
                };
                let progress = append_rows(
                    writer,
                    batch.into_iter().map(|item| {
                        serde_json::json!({
                            "title": item.title, "business_type": item.business_type,
                            "oper_name": item.oper_name, "oper_url": item.oper_url,
                            "oper_ip": item.oper_ip, "status": item.status,
                            "cost_time": item.cost_time, "oper_time": item.oper_time,
                        })
                    }),
                )?;
                Ok(Some(AppendedBatch { last_id, progress }))
            }
            ExportSelection::LoginLogs(filters) => {
                let batch = self
                    .login_infos
                    .find_export_batch(
                        actor,
                        LoginInfoFilter {
                            user_name: filters.user_name(),
                            status: filters.status(),
                            begin_time: filters.begin_time(),
                            end_time: filters.end_time(),
                        },
                        window,
                    )
                    .await?;
                let Some(last_id) = last_batch_id(&batch, window, |item| &item.id)? else {
                    return Ok(None);
                };
                let progress = append_rows(
                    writer,
                    batch.into_iter().map(|item| {
                        serde_json::json!({
                            "user_name": item.user_name, "ipaddr": item.ipaddr,
                            "login_location": item.login_location, "browser": item.browser,
                            "os": item.os, "status": item.status, "msg": item.msg,
                            "login_time": item.login_time,
                        })
                    }),
                )?;
                Ok(Some(AppendedBatch { last_id, progress }))
            }
        }
    }
}

fn append_rows(
    writer: &mut dyn SpreadsheetWriter,
    rows: impl Iterator<Item = serde_json::Value>,
) -> AppResult<SpreadsheetBatchProgress> {
    let mut rows = rows;
    writer.append_rows(&mut rows)
}

fn export_layout(
    selection: &ExportSelection,
) -> (&'static str, &'static [(&'static str, &'static str)]) {
    match selection {
        ExportSelection::Users(_) => ("用户数据", USER_HEADERS),
        ExportSelection::Roles(_) => ("角色数据", ROLE_HEADERS),
        ExportSelection::Posts(_) => ("岗位数据", POST_HEADERS),
        ExportSelection::Configs(_) => ("参数配置", CONFIG_HEADERS),
        ExportSelection::DictTypes(_) => ("字典类型", DICT_TYPE_HEADERS),
        ExportSelection::OperLogs(_) => ("操作日志", OPER_LOG_HEADERS),
        ExportSelection::LoginLogs(_) => ("登录日志", LOGIN_LOG_HEADERS),
    }
}

fn last_batch_id<T>(
    batch: &[T],
    window: ExportCursorWindow,
    id: impl Fn(&T) -> &str,
) -> AppResult<Option<i64>> {
    if batch.is_empty() {
        return Ok(None);
    }
    if batch.len() > window.limit() as usize {
        return Err(AppError::Internal(
            "导出仓储返回了超过窗口大小的批次".into(),
        ));
    }
    let mut cursor = window.after_id();
    for item in batch {
        let current = id(item)
            .parse::<i64>()
            .map_err(|_| AppError::Internal("导出批次主键无效".into()))?;
        if current > window.upper_id() || cursor.is_some_and(|previous| current <= previous) {
            return Err(AppError::Internal("导出主键游标没有严格前进".into()));
        }
        cursor = Some(current);
    }
    Ok(cursor)
}

fn validate_batch_progress(
    batch: &AppendedBatch,
    matched_rows: u64,
    maximum_rows: usize,
) -> AppResult<()> {
    if batch.progress.batch_rows == 0 || batch.progress.batch_rows > EXPORT_BATCH_SIZE {
        return Err(AppError::Internal(
            "Excel 批次写入行数与查询窗口不一致".into(),
        ));
    }
    validate_row_and_byte_limits(
        batch.progress.total_rows,
        batch.progress.total_input_bytes,
        matched_rows,
        maximum_rows,
    )
}

fn validate_runtime_limits(
    execution: &ExportExecution<'_>,
    rows: u64,
    input_bytes: u64,
    maximum_rows: usize,
) -> AppResult<()> {
    if execution.started_at.elapsed() >= StdDuration::from_secs(EXPORT_MAX_RUNTIME_SECONDS as u64) {
        return Err(AppError::Validation(format!(
            "导出执行超过 {EXPORT_MAX_RUNTIME_SECONDS} 秒上限"
        )));
    }
    validate_row_and_byte_limits(rows, input_bytes, execution.matched_rows, maximum_rows)
}

fn validate_row_and_byte_limits(
    rows: u64,
    input_bytes: u64,
    matched_rows: u64,
    maximum_rows: usize,
) -> AppResult<()> {
    let configured_rows =
        u64::try_from(maximum_rows).map_err(|_| AppError::Config("导出行数上限无法转换".into()))?;
    let row_limit = configured_rows.min(EXPORT_BUSINESS_MAX_ROWS as u64);
    if rows > matched_rows || rows > row_limit || rows > SPREADSHEET_MAX_DATA_ROWS {
        return Err(AppError::ExportRowLimitExceeded {
            matched_rows: rows,
            limit: row_limit.min(SPREADSHEET_MAX_DATA_ROWS),
        });
    }
    if input_bytes > EXPORT_MAX_RESULT_BYTES {
        return Err(AppError::PayloadTooLarge(format!(
            "导出内容超过 {} MiB 上限",
            EXPORT_MAX_RESULT_BYTES / 1024 / 1024
        )));
    }
    Ok(())
}

fn validate_artifact_limits(
    artifact: &dyn SpreadsheetArtifact,
    matched_rows: u64,
    maximum_rows: usize,
) -> AppResult<()> {
    validate_row_and_byte_limits(
        artifact.data_rows(),
        artifact.input_bytes(),
        matched_rows,
        maximum_rows,
    )?;
    if artifact.size() > EXPORT_MAX_RESULT_BYTES {
        return Err(AppError::PayloadTooLarge(format!(
            "导出结果文件超过 {} MiB 上限",
            EXPORT_MAX_RESULT_BYTES / 1024 / 1024
        )));
    }
    Ok(())
}

async fn finish_writer_within_deadline(
    writer: Box<dyn SpreadsheetWriter>,
    execution: &ExportExecution<'_>,
) -> AppResult<Box<dyn SpreadsheetArtifact>> {
    let limit = StdDuration::from_secs(EXPORT_MAX_RUNTIME_SECONDS as u64);
    let remaining = limit
        .checked_sub(execution.started_at.elapsed())
        .ok_or_else(|| {
            AppError::Validation(format!("导出执行超过 {EXPORT_MAX_RUNTIME_SECONDS} 秒上限"))
        })?;
    tokio::time::timeout(
        remaining,
        tokio::task::spawn_blocking(move || writer.finish()),
    )
    .await
    .map_err(|_| AppError::Validation(format!("导出执行超过 {EXPORT_MAX_RUNTIME_SECONDS} 秒上限")))?
    .map_err(|error| AppError::Internal(format!("等待 Excel 文件生成失败: {error}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    const _: () = {
        assert!(EXPORT_BATCH_SIZE == 1_000);
        assert!(EXPORT_MAX_RUNTIME_SECONDS == 1_800);
        assert!(EXPORT_MAX_RUNNING_PER_TENANT == 2);
        assert!(EXPORT_MAX_RESULT_BYTES == 512 * 1024 * 1024);
    };

    #[test]
    fn cursor_window_rejects_non_advancing_or_oversized_batches() {
        let window = ExportCursorWindow::new(Some(10), 20, 2);
        assert!(last_batch_id(&["11", "20"], window, |id| id).is_ok());
        assert!(last_batch_id(&["10"], window, |id| id).is_err());
        assert!(last_batch_id(&["12", "11"], window, |id| id).is_err());
        assert!(last_batch_id(&["11", "12", "13"], window, |id| id).is_err());
    }

    #[test]
    fn row_and_byte_limits_fail_closed() {
        validate_row_and_byte_limits(500_000, 512 * 1024 * 1024, 500_000, 500_000)
            .expect("边界值应可用");
        assert!(validate_row_and_byte_limits(500_001, 1, 500_001, 500_000).is_err());
        assert!(validate_row_and_byte_limits(1, 512 * 1024 * 1024 + 1, 1, 500_000).is_err());

        let expired = ExportExecution {
            background_job_id: 1,
            lease_owner: "worker-a",
            started_at: Instant::now()
                .checked_sub(StdDuration::from_secs(1_800))
                .expect("测试时间应可回退"),
            matched_rows: 1,
        };
        assert!(validate_runtime_limits(&expired, 0, 0, 500_000).is_err());
    }

    #[test]
    fn deletion_after_request_may_finish_with_fewer_rows_but_new_ids_are_rejected() {
        validate_row_and_byte_limits(998, 1, 1_000, 500_000)
            .expect("执行前删除应允许实际导出少于申请时匹配数");
        let snapshot = ExportCursorWindow::new(Some(998), 1_000, 1_000);
        assert!(last_batch_id(&["999", "1000"], snapshot, |id| id).is_ok());
        assert!(last_batch_id(&["1001"], snapshot, |id| id).is_err());
    }
}
