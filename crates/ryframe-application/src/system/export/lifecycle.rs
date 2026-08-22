use ryframe_kernel::{ActorContext, AppError, AppResult};
use sha2::{Digest, Sha256};

use crate::{
    EnqueueJob,
    ports::export::{CreateExportRecord, ExportRequesterRecord},
};

use super::*;

impl ExportService {
    /// 原子受理当前申请人的终态导出记录删除，并可靠投递异步清理任务。
    pub async fn delete_for_requester(
        &self,
        actor: &ActorContext,
        mut ids: Vec<i64>,
    ) -> AppResult<ExportDeletionResult> {
        normalize_deletion_ids(&mut ids)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.deletion_persistence.begin().await?;
        let result = async {
            let now = transaction.database_now().await?;
            let removed_unread_count = transaction
                .mark_delete_pending(tenant_id, actor.user_id, &ids, now)
                .await?;
            let trace_context = crate::trace_context::current_trace_context();
            transaction
                .enqueue_cleanup(
                    EnqueueJob {
                        tenant_id: None,
                        schedule_id: None,
                        scheduled_for: Some(now),
                        max_runtime_seconds: None,
                        job_type: EXPORT_CLEANUP_JOB_TYPE.to_owned(),
                        payload: serde_json::json!({"request_version": 1}),
                        priority: -5,
                        available_at: now,
                        max_attempts: self.default_max_attempts,
                        dedupe_key: Some(deletion_cleanup_dedupe_key(
                            tenant_id,
                            actor.user_id,
                            &ids,
                        )),
                        traceparent: trace_context.traceparent,
                        tracestate: trace_context.tracestate,
                    },
                    now,
                )
                .await?;
            Ok::<_, AppError>(removed_unread_count)
        }
        .await;
        let removed_unread_count = match result {
            Ok(removed_unread_count) => {
                transaction.commit().await?;
                if let Some(job_queue) = &self.job_queue {
                    job_queue.notify_background_jobs().await;
                }
                removed_unread_count
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                return Err(error);
            }
        };

        Ok(ExportDeletionResult {
            accepted_count: ids.len() as u64,
            accepted_ids: ids,
            removed_unread_count,
        })
    }

    /// 在同一事务内创建内部 Worker 任务与公开导出任务。
    pub async fn request(
        &self,
        actor: &ActorContext,
        command: RequestExportCommand,
    ) -> AppResult<ExportJobVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        validate_request_command(&command)?;
        let resource = command.selection.resource();
        let (request_actor, authorization_fingerprint) = self
            .users
            .resolve_current_export_authorization(
                tenant_id,
                actor.user_id,
                &command.permission_code,
            )
            .await?;
        let request_fingerprint = calculate_request_fingerprint(
            tenant_id,
            actor.user_id,
            &command.permission_code,
            &command.selection,
            &authorization_fingerprint,
        )?;
        let transaction = self.request_persistence.begin().await?;
        let result = async {
            let now = transaction.database_now().await?;
            if let Some(existing) = transaction
                .find_active(tenant_id, actor.user_id, &request_fingerprint)
                .await?
            {
                return Ok::<_, AppError>((existing, false));
            }
            let snapshot = self
                .summarize_request_selection(
                    transaction.as_ref(),
                    &request_actor,
                    &command.selection,
                )
                .await?;
            let trace_context = crate::trace_context::current_trace_context();
            let background_job_id = transaction
                .enqueue_job(
                    EnqueueJob {
                        tenant_id: Some(tenant_id.to_owned()),
                        schedule_id: None,
                        scheduled_for: Some(now),
                        max_runtime_seconds: Some(EXPORT_MAX_RUNTIME_SECONDS),
                        job_type: EXPORT_JOB_TYPE.to_owned(),
                        payload: serde_json::to_value(ExportJobPayload::new(resource)).map_err(
                            |error| {
                                AppError::Internal(format!("导出后台任务载荷编码失败: {error}"))
                            },
                        )?,
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
            let stored_request = StoredExportRequest {
                request_version: EXPORT_REQUEST_VERSION,
                selection: command.selection,
                authorization_fingerprint,
                snapshot_at: now,
                upper_id: snapshot.upper_id,
                matched_rows: snapshot.matched_rows,
            };
            let export = transaction
                .create_export(
                    CreateExportRecord {
                        tenant_id: tenant_id.to_owned(),
                        requester_id: actor.user_id,
                        resource: resource.to_owned(),
                        background_job_id,
                        request_params: serde_json::to_value(&stored_request).map_err(|error| {
                            AppError::Internal(format!("导出请求编码失败: {error}"))
                        })?,
                        request_version: i32::from(EXPORT_REQUEST_VERSION),
                        permission_code: command.permission_code,
                        authorization_fingerprint: stored_request.authorization_fingerprint,
                        request_fingerprint,
                        snapshot_at: now,
                        upper_id: snapshot.upper_id,
                        matched_rows: i64::try_from(snapshot.matched_rows)
                            .map_err(|_| AppError::Config("导出匹配行数无法写入数据库".into()))?,
                    },
                    now,
                )
                .await?;
            Ok((export, true))
        }
        .await;
        match result {
            Ok((export, inserted)) => {
                transaction.commit().await?;
                if inserted && let Some(job_queue) = &self.job_queue {
                    job_queue.notify_background_jobs().await;
                }
                Ok(export_requester_view(export))
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
        let export = self
            .requester_persistence
            .find(tenant_id, actor.user_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("导出任务不存在或不属于当前用户".into()))?;
        self.users
            .ensure_current_permission(actor, &export.permission_code)
            .await?;
        Ok(export_requester_view(export))
    }

    /// 读取当前申请人仍具备查看权限的最近导出任务。
    pub async fn list_for_requester(&self, actor: &ActorContext) -> AppResult<Vec<ExportJobVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let exports = self
            .requester_persistence
            .list_recent(tenant_id, actor.user_id, 100)
            .await?;
        let mut result = Vec::with_capacity(exports.len());
        for export in exports {
            if self
                .users
                .ensure_current_permission(actor, &export.permission_code)
                .await
                .is_ok()
            {
                result.push(export_requester_view(export));
            }
        }
        Ok(result)
    }

    /// 统计当前申请人尚未查看的导出完成或失败通知。
    pub async fn unread_notification_count(&self, actor: &ActorContext) -> AppResult<u64> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let recent_exports = self
            .requester_persistence
            .list_recent_for_notifications(tenant_id, actor.user_id, 100)
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
                    EXPORT_STATUS_SUCCEEDED | EXPORT_STATUS_FAILED
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
        let now = self.requester_persistence.database_now().await?;
        self.requester_persistence
            .mark_notifications_read(tenant_id, actor.user_id, ids, now)
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
        let export = self
            .requester_persistence
            .find(tenant_id, actor.user_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("导出任务不存在或不属于当前用户".into()))?;
        self.users
            .ensure_current_permission(actor, &export.permission_code)
            .await?;
        let transaction = self.requester_persistence.begin().await?;
        let now = transaction.database_now().await?;
        if !transaction
            .cancel(tenant_id, actor.user_id, id, now)
            .await?
        {
            let _ = transaction.rollback().await;
            return Err(AppError::Conflict(
                "导出任务已完成、已过期或状态已变化".into(),
            ));
        }
        transaction.commit().await?;
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
        let mut export = self
            .requester_persistence
            .find(tenant_id, actor.user_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("导出任务不存在或不属于当前用户".into()))?;
        let (_, current_fingerprint) = self
            .users
            .resolve_current_export_authorization(tenant_id, actor.user_id, &export.permission_code)
            .await?;
        let stored_request: StoredExportRequest =
            serde_json::from_value(std::mem::take(&mut export.request_params))
                .map_err(|_| AppError::Authorization("导出授权记录无效".into()))?;
        stored_request
            .validate(&export.resource)
            .map_err(|_| AppError::Authorization("导出授权记录无效".into()))?;
        stored_request
            .validate_persisted_snapshot(export_requester_snapshot(&export))
            .map_err(|_| AppError::Authorization("导出授权记录无效".into()))?;
        ensure_download_authorization_matches(
            &stored_request.authorization_fingerprint,
            &current_fingerprint,
        )?;
        let now = self.requester_persistence.database_now().await?;
        if export.status != EXPORT_STATUS_SUCCEEDED
            || export.expires_at.is_none_or(|expires_at| expires_at <= now)
        {
            return Err(AppError::Conflict("导出结果尚未就绪或已过期".into()));
        }
        let file_id = export
            .result_file_id
            .ok_or_else(|| AppError::Internal("导出任务缺少结果文件".into()))?;
        let file = self
            .requester_persistence
            .find_download_file(tenant_id, file_id)
            .await?
            .ok_or_else(|| AppError::NotFound("导出结果文件不存在".into()))?;
        Ok(ExportDownloadLocation {
            bucket: file.bucket,
            path: file.storage_path,
        })
    }
}

fn export_requester_view(export: ExportRequesterRecord) -> ExportJobVo {
    ExportJobVo {
        id: export.id.to_string(),
        resource: export.resource,
        status: export.status,
        result_file_name: export.result_file_name,
        content_type: export.content_type,
        file_size: export.file_size,
        expires_at: export.expires_at,
        error_message: export.error_message,
        snapshot_at: export.snapshot_at,
        matched_rows: export.matched_rows,
        created_at: export.created_at,
        updated_at: export.updated_at,
        completed_at: export.completed_at,
        notification_read_at: export.notification_read_at,
    }
}

fn export_requester_snapshot(export: &ExportRequesterRecord) -> PersistedExportSnapshot<'_> {
    PersistedExportSnapshot {
        request_version: export.request_version,
        authorization_fingerprint: &export.authorization_fingerprint,
        snapshot_at: &export.snapshot_at,
        upper_id: export.upper_id,
        matched_rows: export.matched_rows,
    }
}

fn calculate_request_fingerprint(
    tenant_id: &str,
    requester_id: i64,
    permission_code: &str,
    selection: &ExportSelection,
    authorization_fingerprint: &str,
) -> AppResult<String> {
    let selection = serde_json::to_vec(selection)
        .map_err(|error| AppError::Internal(format!("导出筛选指纹编码失败: {error}")))?;
    let mut digest = Sha256::new();
    digest.update(b"ryframe:export-request:v1\0");
    digest.update(tenant_id.as_bytes());
    digest.update([0]);
    digest.update(requester_id.to_be_bytes());
    digest.update(permission_code.as_bytes());
    digest.update([0]);
    digest.update(authorization_fingerprint.as_bytes());
    digest.update([0]);
    digest.update(selection);
    Ok(hex::encode(digest.finalize()))
}

fn normalize_deletion_ids(ids: &mut Vec<i64>) -> AppResult<()> {
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() || ids.len() > 100 || ids.iter().any(|id| *id <= 0) {
        return Err(AppError::Validation(
            "导出任务 ID 排序去重后必须包含 1 到 100 个正整数".into(),
        ));
    }
    Ok(())
}

fn deletion_cleanup_dedupe_key(tenant_id: &str, requester_id: i64, ids: &[i64]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"ryframe:export-deletion-cleanup:v1\0");
    digest.update(tenant_id.as_bytes());
    digest.update([0]);
    digest.update(requester_id.to_be_bytes());
    for id in ids {
        digest.update(id.to_be_bytes());
    }
    format!("export:delete:{}", hex::encode(digest.finalize()))
}

#[cfg(test)]
mod deletion_tests {
    use super::*;

    fn requester_record() -> ExportRequesterRecord {
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-21T00:00:00Z")
            .expect("测试时间应有效")
            .with_timezone(&chrono::Utc);
        ExportRequesterRecord {
            id: 42,
            resource: "users".into(),
            status: EXPORT_STATUS_SUCCEEDED.into(),
            result_file_name: Some("users-42.xlsx".into()),
            content_type: Some(
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
            ),
            file_size: Some(128),
            expires_at: Some(now),
            error_message: None,
            snapshot_at: now,
            matched_rows: 8,
            created_at: now,
            updated_at: now,
            completed_at: Some(now),
            notification_read_at: None,
            permission_code: "system:user:export".into(),
            request_params: serde_json::json!({"request_version": EXPORT_REQUEST_VERSION}),
            request_version: i32::from(EXPORT_REQUEST_VERSION),
            authorization_fingerprint: "authorization".into(),
            upper_id: 99,
            result_file_id: Some(7),
        }
    }

    #[test]
    fn requester_record_maps_without_database_types() {
        let record = requester_record();
        let snapshot = export_requester_snapshot(&record);
        assert_eq!(snapshot.request_version, i32::from(EXPORT_REQUEST_VERSION));
        assert_eq!(snapshot.authorization_fingerprint, "authorization");
        assert_eq!(snapshot.upper_id, 99);
        assert_eq!(snapshot.matched_rows, 8);

        let view = export_requester_view(record);
        assert_eq!(view.id, "42");
        assert_eq!(view.status, EXPORT_STATUS_SUCCEEDED);
        assert_eq!(view.result_file_name.as_deref(), Some("users-42.xlsx"));
    }

    #[test]
    fn deletion_ids_are_sorted_deduplicated_and_bounded() {
        let mut ids = vec![9, 3, 9, 5];
        normalize_deletion_ids(&mut ids).expect("有效 ID 应通过");
        assert_eq!(ids, vec![3, 5, 9]);

        assert!(normalize_deletion_ids(&mut Vec::new()).is_err());
        assert!(normalize_deletion_ids(&mut vec![0]).is_err());
        let mut too_many = (1..=101).collect::<Vec<_>>();
        assert!(normalize_deletion_ids(&mut too_many).is_err());

        assert_eq!(
            deletion_cleanup_dedupe_key("tenant-a", 7, &[3, 5, 9]),
            deletion_cleanup_dedupe_key("tenant-a", 7, &[3, 5, 9])
        );
        assert_ne!(
            deletion_cleanup_dedupe_key("tenant-a", 7, &[3, 5, 9]),
            deletion_cleanup_dedupe_key("tenant-a", 7, &[3, 5, 10])
        );
    }

    #[test]
    fn request_fingerprint_is_stable_and_authorization_sensitive() {
        let selection = ExportSelection::Roles(RoleExportFilter::new(
            Some(" ops ".into()),
            None,
            Some("0".into()),
        ));
        let first =
            calculate_request_fingerprint("tenant-a", 7, "system:role:export", &selection, "a")
                .expect("指纹应生成");
        let same =
            calculate_request_fingerprint("tenant-a", 7, "system:role:export", &selection, "a")
                .expect("同一输入应生成指纹");
        let changed =
            calculate_request_fingerprint("tenant-a", 7, "system:role:export", &selection, "b")
                .expect("变更授权仍应生成指纹");
        assert_eq!(first, same);
        assert_ne!(first, changed);
        let other_resource =
            ExportSelection::Configs(ConfigExportFilter::new(Some("ops".into()), None));
        for different in [
            calculate_request_fingerprint("tenant-b", 7, "system:role:export", &selection, "a"),
            calculate_request_fingerprint("tenant-a", 8, "system:role:export", &selection, "a"),
            calculate_request_fingerprint("tenant-a", 7, "system:other:export", &selection, "a"),
            calculate_request_fingerprint(
                "tenant-a",
                7,
                "system:role:export",
                &other_resource,
                "a",
            ),
        ] {
            assert_ne!(first, different.expect("不同输入仍应生成指纹"));
        }
        assert_eq!(first.len(), 64);
    }
}
