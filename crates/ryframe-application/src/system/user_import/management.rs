impl UserImportService {
    pub fn new(
        queue: Arc<JobQueue>,
        user: Arc<UserService>,
        file: Arc<FileService>,
        spreadsheets: Arc<dyn SpreadsheetDocumentProcessor>,
        persistence: Arc<dyn UserImportPersistencePort>,
        config: crate::UserImportPolicy,
    ) -> Self {
        Self {
            queue,
            user,
            file,
            hash_permits: Arc::new(Semaphore::new(config.hash_parallelism)),
            spreadsheets,
            persistence,
            config,
        }
    }

    pub fn upload_policy(&self) -> UploadPolicy {
        UploadPolicy {
            max_file_size: u64::try_from(self.config.max_file_bytes).unwrap_or(u64::MAX),
            allowed_extensions: vec!["xlsx".to_owned()],
        }
    }

    /// 在阻塞线程中校验导入表头，并把原始字节所有权交还给后续上传步骤。
    pub async fn validate_source(&self, data: Vec<u8>) -> AppResult<Vec<u8>> {
        self.spreadsheets
            .validate_source(data, UserImportData::excel_headers())
            .await
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
        self.file
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
            .user
            .resolve_current_authorization(tenant_id, actor.user_id, USER_IMPORT_PERMISSION)
            .await?;
        let directory = self.load_department_directory(tenant_id).await?;
        let available_paths = directory.available_paths(&authorization.actor)?;
        self.spreadsheets
            .export_template(
                "用户数据",
                UserImportData::excel_headers(),
                "可用部门",
                "部门完整路径",
                available_paths,
            )
            .await
    }

    pub async fn find_by_idempotency(
        &self,
        actor: &ActorContext,
        idempotency_key_hash: &str,
    ) -> AppResult<Option<UserImportJobVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        validate_sha256("幂等键", idempotency_key_hash)?;
        let transaction = self.persistence.begin().await?;
        let existing = transaction
            .find_by_idempotency(tenant_id, idempotency_key_hash)
            .await?;
        let Some(existing) = existing else {
            transaction.rollback().await?;
            return Ok(None);
        };
        let requester_username = transaction
            .requester_username(tenant_id, existing.requester_user_id)
            .await?;
        let job = job_vo_with_requester(existing, requester_username);
        // 幂等重放同样属于成功写请求；短事务绑定审计，避免产生 transaction_unbound 告警。
        transaction.commit().await?;
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

        let transaction = self.persistence.begin().await?;
        transaction.lock_tenant(tenant_id).await?;
        if let Some(existing) = transaction
            .find_by_idempotency(tenant_id, &command.idempotency_key_hash)
            .await?
        {
            let requester_username = transaction
                .requester_username(tenant_id, existing.requester_user_id)
                .await?;
            let job = job_vo_with_requester(existing, requester_username);
            transaction.commit().await?;
            return Ok(RequestUserImportOutcome {
                job,
                inserted: false,
            });
        }
        let active = transaction.active_count(tenant_id).await?;
        if active
            >= u64::try_from(self.config.max_active_per_tenant)
                .map_err(|_| AppError::Config("用户导入活动任务上限无效".into()))?
        {
            return Err(AppError::Conflict(
                "当前租户已有进行中的用户导入任务".into(),
            ));
        }

        let import_id = next_id()?;
        let now = transaction.database_now().await?;
        let source_file = transaction
            .lock_source(tenant_id, command.source_file_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入源文件不存在或已被回收".into()))?;
        if validate_import_source(&source_file, &command.source_sha256)?
            && !transaction
                .restore_source(tenant_id, command.source_file_id, now)
                .await?
        {
            return Err(AppError::NotFound(
                "用户导入源文件已进入最终回收阶段".into(),
            ));
        }
        let trace_context = crate::trace_context::current_trace_context();
        let queued = transaction
            .enqueue(EnqueueJob {
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
            })
            .await?;
        let job = transaction
            .create(
                NewUserImportJob {
                    id: import_id,
                    tenant_id: tenant_id.to_owned(),
                    requester_user_id: actor.user_id,
                    background_job_id: queued.job_id,
                    idempotency_key_hash: command.idempotency_key_hash,
                    source_file_id: command.source_file_id,
                    source_name: command.source_name,
                    source_sha256: command.source_sha256,
                },
                now,
            )
            .await?;
        transaction.commit().await?;
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
        let transaction = self.persistence.begin().await?;
        let result: AppResult<bool> = async {
            transaction.lock_tenant(tenant_id).await?;
            let now = transaction.database_now().await?;
            let Some(file) = transaction
                .lock_source(tenant_id, source_file_id)
                .await?
            else {
                return Ok(false);
            };
            if file.bucket != IMPORT_BUCKET {
                return Err(AppError::Validation("只能清理用户导入专用文件".into()));
            }
            transaction
                .mark_source_for_cleanup(
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
            Ok(true) => transaction.commit().await,
            Ok(false) => transaction.rollback().await,
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
        let page = self
            .persistence
            .list(
                tenant_id,
                params.page,
                UserImportReadFilter {
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
        let requester_usernames = self
            .persistence
            .requester_usernames(tenant_id, &requester_ids)
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
        let job = self
            .persistence
            .find(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        let requester_username = self
            .persistence
            .requester_usernames(tenant_id, &[job.requester_user_id])
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
        let rows = self
            .persistence
            .rows(tenant_id, id, page)
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
        if !self.persistence.request_cancel(tenant_id, id).await? {
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
        let job = self
            .persistence
            .find(tenant_id, id)
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
        self.file
            .download_by_id(actor, file_id, IMPORT_BUCKET)
            .await
    }

    async fn ensure_visible(&self, tenant_id: &str, id: i64) -> AppResult<()> {
        self.persistence
            .find(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        Ok(())
    }
}

pub fn validate_import_source(
    source: &UserImportSourceRecord,
    expected_sha256: &str,
) -> AppResult<bool> {
    if source.bucket != IMPORT_BUCKET {
        return Err(AppError::Validation(
            "用户导入源文件存储边界不匹配".into(),
        ));
    }
    if source.sha256 != expected_sha256 {
        return Err(AppError::Validation("用户导入源文件摘要不匹配".into()));
    }
    match source.state {
        UserImportSourceState::Ready => Ok(false),
        UserImportSourceState::Recoverable => Ok(true),
        UserImportSourceState::Unavailable => {
            Err(AppError::Validation("用户导入源文件尚未完成上传".into()))
        }
    }
}
