use ryframe_db::{CreateExportJob, EnqueueBackgroundJob, ReadConsistency, entities::export_job};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::TransactionTrait;

use super::*;

impl ExportService {
    /// 在同一事务内创建内部 Worker 任务与公开导出任务。
    pub async fn request(
        &self,
        actor: &ActorContext,
        command: RequestExportCommand,
    ) -> AppResult<ExportJobVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        validate_request_command(&command)?;
        self.users
            .resolve_current_export_authorization(
                tenant_id,
                actor.user_id,
                &command.permission_code,
            )
            .await?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let result = async {
            let now = self.background_jobs.database_utc_now(&transaction).await?;
            let trace_context = crate::trace_context::current_trace_context();
            let job = self
                .background_jobs
                .enqueue_in_transaction(
                    &transaction,
                    EnqueueBackgroundJob {
                        tenant_id: Some(tenant_id.to_owned()),
                        schedule_id: None,
                        scheduled_for: Some(now),
                        max_runtime_seconds: None,
                        job_type: EXPORT_JOB_TYPE.to_owned(),
                        payload: serde_json::json!({ "resource": command.resource }),
                        priority: 5,
                        available_at: now,
                        max_attempts: self.default_max_attempts,
                        dedupe_key: None,
                        traceparent: trace_context.traceparent,
                        tracestate: trace_context.tracestate,
                    },
                    now,
                )
                .await?;
            self.exports
                .create_in_transaction(
                    &transaction,
                    CreateExportJob {
                        tenant_id: tenant_id.to_owned(),
                        requester_id: actor.user_id,
                        resource: command.resource,
                        background_job_id: job.job.id,
                        request_params: serde_json::to_value(StoredExportRequest {
                            request: command.request_params,
                            authorization_fingerprint: None,
                        })
                        .map_err(|error| {
                            AppError::Internal(format!("导出请求编码失败: {error}"))
                        })?,
                        permission_code: command.permission_code,
                    },
                    now,
                )
                .await
        }
        .await;
        match result {
            Ok(export) => {
                crate::commit_current_audit(transaction).await?;
                if let Some(job_queue) = &self.job_queue {
                    job_queue.notify_background_jobs().await;
                }
                Ok(ExportJobVo::from(export))
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    /// 读取申请人自己的导出任务。
    pub async fn find_for_requester(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<ExportJobVo> {
        validate_job_id(id)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let export = self
            .exports
            .find_by_id_for_requester(&db, tenant_id, actor.user_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("导出任务不存在或不属于当前用户".into()))?;
        self.users
            .ensure_current_permission(actor, &export.permission_code)
            .await?;
        Ok(ExportJobVo::from(export))
    }

    /// 读取当前申请人仍具备查看权限的最近导出任务。
    pub async fn list_for_requester(&self, actor: &ActorContext) -> AppResult<Vec<ExportJobVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        let exports = self
            .exports
            .list_for_requester(&db, tenant_id, actor.user_id, 100)
            .await?;
        let mut result = Vec::with_capacity(exports.len());
        for export in exports {
            if self
                .users
                .ensure_current_permission(actor, &export.permission_code)
                .await
                .is_ok()
            {
                result.push(ExportJobVo::from(export));
            }
        }
        Ok(result)
    }

    /// 统计当前申请人尚未查看的导出完成或失败通知。
    pub async fn unread_notification_count(&self, actor: &ActorContext) -> AppResult<u64> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let recent_exports = self
            .exports
            .list_for_requester(&db, tenant_id, actor.user_id, 100)
            .await?;
        let authorization = self
            .users
            .calculate_current_authorization(tenant_id, actor.user_id)
            .await?;
        if !authorization.tenant.is_available(chrono::Utc::now())
            || !authorization.user.is_enabled()
        {
            return Err(AppError::Authorization(
                "导出申请人的账号或租户已不可用".into(),
            ));
        }
        Ok(recent_exports
            .iter()
            .filter(|export| {
                matches!(
                    export.status.as_str(),
                    export_job::Model::STATUS_SUCCEEDED | export_job::Model::STATUS_FAILED
                ) && export.notification_read_at.is_none()
                    && (authorization.actor.is_super_admin
                        || ryframe_auth::rbac::has_permission(
                            &authorization.permission_codes,
                            &export.permission_code,
                        ))
            })
            .count() as u64)
    }

    /// 幂等确认当前申请人已经实际看到的导出完成或失败通知。
    pub async fn mark_notifications_read(
        &self,
        actor: &ActorContext,
        ids: &[i64],
    ) -> AppResult<u64> {
        if ids.is_empty() || ids.len() > 100 || ids.iter().any(|id| *id <= 0) {
            return Err(AppError::Validation(
                "导出通知 ID 数量必须介于 1 和 100 之间且均为正整数".into(),
            ));
        }
        let tenant_id = crate::validated_tenant_id(actor)?;
        let now = self
            .background_jobs
            .database_utc_now(self.db.write())
            .await?;
        self.exports
            .mark_notifications_read(self.db.write(), tenant_id, actor.user_id, ids, now)
            .await
    }

    /// 取消申请人自己的尚未完成导出任务。
    pub async fn cancel_for_requester(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<ExportJobVo> {
        validate_job_id(id)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let export = self
            .exports
            .find_by_id_for_requester(&db, tenant_id, actor.user_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("导出任务不存在或不属于当前用户".into()))?;
        self.users
            .ensure_current_permission(actor, &export.permission_code)
            .await?;
        let now = self
            .background_jobs
            .database_utc_now(self.db.write())
            .await?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        if !self
            .exports
            .cancel_for_requester(&transaction, tenant_id, actor.user_id, id, now)
            .await?
        {
            let _ = transaction.rollback().await;
            return Err(AppError::Conflict(
                "导出任务已完成、已过期或状态已变化".into(),
            ));
        }
        crate::commit_current_audit(transaction).await?;
        self.find_for_requester(actor, id).await
    }

    /// 返回当前申请人尚未过期的导出结果位置。
    pub async fn download_location_for_requester(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<ExportDownloadLocation> {
        validate_job_id(id)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let export = self
            .exports
            .find_by_id_for_requester(&db, tenant_id, actor.user_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("导出任务不存在或不属于当前用户".into()))?;
        let (_, current_fingerprint) = self
            .users
            .resolve_current_export_authorization(tenant_id, actor.user_id, &export.permission_code)
            .await?;
        let stored_request: StoredExportRequest =
            serde_json::from_value(export.request_params.clone())
                .map_err(|_| AppError::Authorization("导出授权记录无效".into()))?;
        ensure_download_authorization_matches(
            stored_request.authorization_fingerprint.as_deref(),
            &current_fingerprint,
        )?;
        let now = self
            .background_jobs
            .database_utc_now(self.db.write())
            .await?;
        if export.status != export_job::Model::STATUS_SUCCEEDED
            || export.expires_at.is_none_or(|expires_at| expires_at <= now)
        {
            return Err(AppError::Conflict("导出结果尚未就绪或已过期".into()));
        }
        let file_id = export
            .result_file_id
            .ok_or_else(|| AppError::Internal("导出任务缺少结果文件".into()))?;
        let file = self
            .files
            .find_by_id(self.db.write(), tenant_id, file_id)
            .await?
            .ok_or_else(|| AppError::NotFound("导出结果文件不存在".into()))?;
        Ok(ExportDownloadLocation {
            bucket: file.bucket,
            path: file.storage_path,
        })
    }
}
