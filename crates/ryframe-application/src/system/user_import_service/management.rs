impl UserImportService {
    pub fn new(
        db: ControlDatabaseCluster,
        queue: Arc<JobQueue>,
        user_service: Arc<UserService>,
        file_service: Arc<FileService>,
        config: UserImportConfig,
    ) -> Self {
        Self {
            db,
            queue,
            user_service,
            file_service,
            hash_permits: Arc::new(Semaphore::new(config.hash_parallelism)),
            config,
        }
    }

    pub fn upload_policy(&self) -> UploadPolicy {
        UploadPolicy {
            max_file_size: u64::try_from(self.config.max_file_bytes).unwrap_or(u64::MAX),
            allowed_extensions: vec!["xlsx".to_owned()],
        }
    }

    /// 上传导入源文件，但把当前 HTTP 请求的最终操作审计留给导入任务创建事务。
    pub async fn upload_source(
        &self,
        actor: &ActorContext,
        original_name: String,
        data: Vec<u8>,
    ) -> AppResult<UploadResponse> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let policy = self.upload_policy();
        self.file_service
            .upload_internal_unbound(
                tenant_id,
                &actor.username,
                UploadCommand {
                    original_name,
                    data,
                    policy: &policy,
                    bucket: IMPORT_BUCKET,
                    compress: false,
                },
            )
            .await
    }

    /// 按申请人的当前主库授权生成不含内部标识的用户导入模板。
    pub async fn build_template(&self, actor: &ActorContext) -> AppResult<Vec<u8>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let authorization = self
            .user_service
            .resolve_current_authorization(tenant_id, actor.user_id, USER_IMPORT_PERMISSION)
            .await?;
        let directory = self.load_department_directory(tenant_id).await?;
        let available_paths = directory.available_paths(&authorization.actor)?;
        tokio::task::spawn_blocking(move || {
            ExcelExporter::export_template_with_reference(
                "用户数据",
                UserImportData::excel_headers(),
                "可用部门",
                "部门完整路径",
                &available_paths,
            )
        })
        .await
        .map_err(|error| AppError::Internal(format!("用户导入模板生成任务异常结束: {error}")))?
    }

    pub async fn find_by_idempotency(
        &self,
        actor: &ActorContext,
        idempotency_key_hash: &str,
    ) -> AppResult<Option<UserImportJobVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        validate_sha256("幂等键", idempotency_key_hash)?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let existing = UserImportRepository
            .find_by_idempotency_in_txn(&transaction, tenant_id, idempotency_key_hash)
            .await?;
        let Some(existing) = existing else {
            transaction.rollback().await.map_err(database_error)?;
            return Ok(None);
        };
        let requester_username = UserRepository
            .find_usernames_by_ids(&transaction, tenant_id, &[existing.requester_user_id])
            .await?
            .into_iter()
            .next()
            .map(|(_, username)| username);
        let job = job_vo_with_requester(existing, requester_username);
        // 幂等重放同样属于成功写请求；短事务绑定审计，避免产生 transaction_unbound 告警。
        crate::commit_current_audit(transaction).await?;
        Ok(Some(job))
    }

    pub async fn request(
        &self,
        actor: &ActorContext,
        command: RequestUserImportCommand,
    ) -> AppResult<RequestUserImportOutcome> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        validate_sha256("幂等键", &command.idempotency_key_hash)?;
        validate_sha256("源文件", &command.source_sha256)?;
        if command.source_file_id <= 0 {
            return Err(AppError::Validation("导入源文件标识无效".into()));
        }
        if command.source_name.is_empty() || command.source_name.len() > 255 {
            return Err(AppError::Validation(
                "导入文件名长度必须介于 1 和 255 字节之间".into(),
            ));
        }

        let transaction = self.db.write().begin().await.map_err(database_error)?;
        TenantRepository
            .lock_tenant_in_txn(&transaction, tenant_id)
            .await?;
        if let Some(existing) = UserImportRepository
            .find_by_idempotency_in_txn(&transaction, tenant_id, &command.idempotency_key_hash)
            .await?
        {
            let requester_username = UserRepository
                .find_usernames_by_ids(&transaction, tenant_id, &[existing.requester_user_id])
                .await?
                .into_iter()
                .next()
                .map(|(_, username)| username);
            let job = job_vo_with_requester(existing, requester_username);
            crate::commit_current_audit(transaction).await?;
            return Ok(RequestUserImportOutcome {
                job,
                inserted: false,
            });
        }
        let active = UserImportRepository
            .count_active_in_txn(&transaction, tenant_id)
            .await?;
        if active
            >= u64::try_from(self.config.max_active_per_tenant)
                .map_err(|_| AppError::Config("用户导入活动任务上限无效".into()))?
        {
            return Err(AppError::Conflict(
                "当前租户已有进行中的用户导入任务".into(),
            ));
        }

        let import_id = try_next_snowflake_id()?;
        let now = UserImportRepository.database_utc_now(&transaction).await?;
        let source_file = FileRepository
            .find_by_id_any_status_for_update(&transaction, tenant_id, command.source_file_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入源文件不存在或已被回收".into()))?;
        if source_file.bucket != IMPORT_BUCKET {
            return Err(AppError::Validation("用户导入源文件存储边界不匹配".into()));
        }
        if source_file.file_sha256 != command.source_sha256 {
            return Err(AppError::Validation("用户导入源文件摘要不匹配".into()));
        }
        if source_file.upload_status == ryframe_db::entities::sys_file::Model::UPLOAD_STATUS_CLEANUP
        {
            if !FileRepository
                .restore_import_file_for_reference_in_txn(
                    &transaction,
                    tenant_id,
                    command.source_file_id,
                    now,
                )
                .await?
            {
                return Err(AppError::NotFound(
                    "用户导入源文件已进入最终回收阶段".into(),
                ));
            }
        } else if source_file.upload_status
            != ryframe_db::entities::sys_file::Model::UPLOAD_STATUS_READY
            || source_file.del_flag != ryframe_db::entities::sys_file::Model::DEL_FLAG_NORMAL
        {
            return Err(AppError::Validation("用户导入源文件尚未完成上传".into()));
        }
        let trace_context = crate::trace_context::current_trace_context();
        let queued = self
            .queue
            .enqueue_in_transaction(
                &transaction,
                EnqueueBackgroundJob {
                    tenant_id: Some(tenant_id.to_owned()),
                    schedule_id: None,
                    scheduled_for: Some(now),
                    max_runtime_seconds: Some(USER_IMPORT_MAX_RUNTIME_SECONDS),
                    job_type: USER_IMPORT_JOB_TYPE.to_owned(),
                    payload: serde_json::json!({ "import_job_id": import_id.to_string() }),
                    priority: 0,
                    available_at: now,
                    max_attempts: USER_IMPORT_MAX_ATTEMPTS,
                    dedupe_key: Some(format!("{tenant_id}:{}", command.idempotency_key_hash)),
                    traceparent: trace_context.traceparent,
                    tracestate: trace_context.tracestate,
                },
            )
            .await?;
        let job = UserImportRepository
            .create_in_txn(
                &transaction,
                CreateUserImportJob {
                    id: import_id,
                    tenant_id: tenant_id.to_owned(),
                    requester_user_id: actor.user_id,
                    background_job_id: queued.job.id,
                    idempotency_key_hash: command.idempotency_key_hash,
                    source_file_id: command.source_file_id,
                    source_name_snapshot: command.source_name,
                    source_sha256: command.source_sha256,
                },
                now,
            )
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.queue.notify_background_jobs().await;
        Ok(RequestUserImportOutcome {
            job: job_vo_with_requester(job, Some(actor.username.clone())),
            inserted: true,
        })
    }

    /// 导入任务创建失败后，将本次上传且尚未被任何任务引用的文件纳入延迟回收。
    pub async fn schedule_unreferenced_source_cleanup(
        &self,
        actor: &ActorContext,
        source_file_id: i64,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let result: AppResult<bool> = async {
            TenantRepository
                .lock_tenant_in_txn(&transaction, tenant_id)
                .await?;
            let now = FileRepository.database_utc_now(&transaction).await?;
            let Some(file) = FileRepository
                .find_by_id_any_status_for_update(&transaction, tenant_id, source_file_id)
                .await?
            else {
                return Ok(false);
            };
            if file.bucket != IMPORT_BUCKET {
                return Err(AppError::Validation("只能清理用户导入专用文件".into()));
            }
            FileRepository
                .mark_import_orphan_for_cleanup_in_txn(
                    &transaction,
                    tenant_id,
                    source_file_id,
                    now,
                    now + chrono::Duration::minutes(IMPORT_ORPHAN_CLEANUP_GRACE_MINUTES),
                )
                .await
        }
        .await;
        match result {
            // 该事务只负责失败补偿，不能把主请求提前标记为审计成功。
            Ok(true) => transaction.commit().await.map_err(database_error),
            Ok(false) => transaction.rollback().await.map_err(database_error),
            Err(error) => {
                if let Err(rollback_error) = transaction.rollback().await {
                    tracing::error!(%rollback_error, "用户导入孤儿文件回收事务回滚失败");
                }
                Err(error)
            }
        }
    }

    pub async fn list(
        &self,
        actor: &ActorContext,
        params: UserImportListParams,
    ) -> AppResult<PageResult<UserImportJobVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let status = normalize_status(params.status.as_deref())?;
        let page = UserImportRepository
            .list_for_tenant(
                self.db.write(),
                tenant_id,
                &params.page,
                UserImportFilter {
                    status: status.as_deref(),
                },
            )
            .await?;
        let mut requester_ids = page
            .records
            .iter()
            .map(|job| job.requester_user_id)
            .collect::<Vec<_>>();
        requester_ids.sort_unstable();
        requester_ids.dedup();
        let requester_usernames = UserRepository
            .find_usernames_by_ids(self.db.write(), tenant_id, &requester_ids)
            .await?
            .into_iter()
            .collect::<HashMap<_, _>>();
        Ok(PageResult::new(
            page.records
                .into_iter()
                .map(|job| {
                    let requester_username =
                        requester_usernames.get(&job.requester_user_id).cloned();
                    job_vo_with_requester(job, requester_username)
                })
                .collect(),
            page.total,
            &params.page,
        ))
    }

    pub async fn get(&self, actor: &ActorContext, id: i64) -> AppResult<UserImportJobVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let job = UserImportRepository
            .find_by_id_for_tenant(self.db.write(), tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        let requester_username = UserRepository
            .find_usernames_by_ids(self.db.write(), tenant_id, &[job.requester_user_id])
            .await?
            .into_iter()
            .next()
            .map(|(_, username)| username);
        Ok(job_vo_with_requester(job, requester_username))
    }

    pub async fn rows(
        &self,
        actor: &ActorContext,
        id: i64,
        page: ValidatedPageQuery,
    ) -> AppResult<PageResult<UserImportRowVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_visible(tenant_id, id).await?;
        let rows = UserImportRepository
            .list_row_results(self.db.write(), tenant_id, id, &page)
            .await?;
        Ok(PageResult::new(
            rows.records
                .into_iter()
                .map(UserImportRowVo::from)
                .collect(),
            rows.total,
            &page,
        ))
    }

    pub async fn cancel(&self, actor: &ActorContext, id: i64) -> AppResult<UserImportJobVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_visible(tenant_id, id).await?;
        let now = UserImportRepository
            .database_utc_now(self.db.write())
            .await?;
        if !UserImportRepository
            .request_cancel(self.db.write(), tenant_id, id, now)
            .await?
        {
            return Err(AppError::Conflict("用户导入任务已结束或状态已变化".into()));
        }
        self.get(actor, id).await
    }

    pub async fn download_report(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<DownloadedFile> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let job = UserImportRepository
            .find_by_id_for_tenant(self.db.write(), tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        if !job.is_terminal() {
            return Err(AppError::Conflict("用户导入报告尚未生成".into()));
        }
        let file_id = job.error_report_file_id.ok_or_else(|| {
            if job.failure_count == 0 && job.skipped_count == 0 {
                AppError::NotFound("该导入任务没有失败或跳过记录".into())
            } else {
                AppError::Conflict("用户导入报告尚未就绪".into())
            }
        })?;
        self.file_service
            .download_by_id(actor, file_id, IMPORT_BUCKET)
            .await
    }

    async fn ensure_visible(&self, tenant_id: &str, id: i64) -> AppResult<()> {
        UserImportRepository
            .find_by_id_for_tenant(self.db.write(), tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        Ok(())
    }
}
