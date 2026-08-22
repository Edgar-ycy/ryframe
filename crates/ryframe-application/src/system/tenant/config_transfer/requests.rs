use super::*;

impl TenantConfigTransferService {
    pub async fn request_package_export(
        &self,
        actor: &ActorContext,
        idempotency_key_hash: &str,
    ) -> AppResult<RequestTenantConfigBundleOutcome> {
        validate_sha256(idempotency_key_hash)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        let operation = async {
            transaction
                .lock_tenant_configuration(tenant_id, None)
                .await?;
            let now = transaction.database_now().await?;
            let tenant_name = transaction.tenant_name(tenant_id).await?;
            if let Some(bundle) = transaction
                .find_bundle_by_idempotency_key(tenant_id, actor.user_id, idempotency_key_hash)
                .await?
            {
                return Ok::<_, AppError>((bundle, false));
            }
            let proposed_bundle_id = next_id()?;
            let trace = crate::trace_context::current_trace_context();
            let enqueued = self
                .queue
                .enqueue_in_transaction(
                    transaction.background_jobs(),
                    EnqueueJob {
                        tenant_id: Some(tenant_id.to_owned()),
                        schedule_id: None,
                        scheduled_for: Some(now),
                        max_runtime_seconds: Some(self.max_runtime_seconds()?),
                        job_type: TENANT_CONFIG_EXPORT_JOB_TYPE.to_owned(),
                        payload: json!({ "bundle_id": proposed_bundle_id.to_string() }),
                        priority: -5,
                        available_at: now,
                        max_attempts: MAX_ATTEMPTS,
                        dedupe_key: Some(format!(
                            "{tenant_id}:{}:export:{idempotency_key_hash}",
                            actor.user_id
                        )),
                        traceparent: trace.traceparent,
                        tracestate: trace.tracestate,
                    },
                )
                .await?;
            let bundle = if enqueued.inserted {
                transaction
                    .insert_bundle(TenantConfigBundleRecord {
                        id: proposed_bundle_id,
                        tenant_id: tenant_id.to_owned(),
                        origin: TenantConfigBundleRecord::ORIGIN_GENERATED.to_owned(),
                        source_tenant_key: tenant_id.to_owned(),
                        source_tenant_name_snapshot: tenant_name,
                        package_schema_version: TENANT_CONFIG_PACKAGE_SCHEMA.to_owned(),
                        source_app_version: env!("CARGO_PKG_VERSION").to_owned(),
                        file_id: None,
                        sha256: None,
                        resource_counts: json!({}),
                        item_count: 0,
                        status: TenantConfigBundleRecord::STATUS_PENDING.to_owned(),
                        background_job_id: Some(enqueued.job_id),
                        idempotency_key_hash: Some(idempotency_key_hash.to_owned()),
                        created_by: actor.user_id,
                        error_summary: None,
                        expires_at: Some(
                            now + Duration::hours(i64::from(self.config.artifact_hours)),
                        ),
                        created_at: now,
                        updated_at: now,
                    })
                    .await?
            } else {
                transaction
                    .lock_bundle_by_background_job(enqueued.job_id)
                    .await?
                    .ok_or_else(|| AppError::Conflict("配置包导出幂等记录尚未完成".into()))?
            };
            Ok::<_, AppError>((bundle, enqueued.inserted))
        }
        .await;
        match operation {
            Ok((bundle, inserted)) => {
                transaction.commit_audited().await?;
                self.queue.notify_background_jobs().await;
                Ok(RequestTenantConfigBundleOutcome {
                    bundle: bundle.into(),
                    inserted,
                })
            }
            Err(error) => {
                transaction.rollback().await?;
                Err(error)
            }
        }
    }

    pub async fn upload_package_and_create_transfer(
        &self,
        actor: &ActorContext,
        original_name: String,
        data: Vec<u8>,
        idempotency_key_hash: &str,
    ) -> AppResult<RequestTenantConfigTransferOutcome> {
        validate_sha256(idempotency_key_hash)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let (parsed, data) =
            crate::system::tenant::config_package::parse_tenant_config_package_with_source(
                Arc::clone(&self.archive),
                data,
                self.package_limits(),
            )
            .await?;
        if let Some((existing, bundle)) = self
            .find_uploaded_transfer_by_idempotency_and_bind_audit(
                tenant_id,
                actor.user_id,
                idempotency_key_hash,
                &parsed.package_sha256,
                &parsed.manifest.required_capabilities,
            )
            .await?
        {
            return Ok(RequestTenantConfigTransferOutcome {
                transfer: TenantConfigTransferVo::from_models(existing, &bundle)?,
                inserted: false,
            });
        }
        let uploaded = self
            .file
            .upload_config_package_unbound(
                tenant_id,
                &actor.username,
                original_name,
                data,
                u64::try_from(self.config.max_package_bytes).unwrap_or(u64::MAX),
            )
            .await?;
        let file_id = parse_file_id(&uploaded.file_id)?;
        let result = self
            .insert_uploaded_bundle_and_transfer(actor, file_id, parsed, idempotency_key_hash)
            .await;
        if match result.as_ref() {
            Ok(outcome) => !outcome.inserted,
            Err(_) => true,
        } {
            let _ = self
                .file
                .schedule_unreferenced_config_package_cleanup(tenant_id, file_id)
                .await;
        }
        result
    }

    async fn find_uploaded_transfer_by_idempotency_and_bind_audit(
        &self,
        tenant_id: &str,
        requested_by: i64,
        idempotency_key_hash: &str,
        package_sha256: &str,
        required_capabilities: &[CapabilityRequirement],
    ) -> AppResult<Option<(TenantConfigTransferRecord, TenantConfigBundleRecord)>> {
        let transaction = self.persistence.begin().await?;
        let result = async {
            transaction
                .lock_tenant_configuration(tenant_id, None)
                .await?;
            self.product
                .ensure_capability_requirements_in_txn(
                    transaction.product(),
                    tenant_id,
                    required_capabilities,
                )
                .await?;
            let existing = transaction
                .find_transfer_by_idempotency_key(tenant_id, requested_by, idempotency_key_hash)
                .await?;
            let Some(existing) = existing else {
                return Ok::<_, AppError>(None);
            };
            ensure_transfer_request_identity(&existing, REQUEST_KIND_UPLOAD, package_sha256)?;
            let bundle = transaction
                .lock_bundle(tenant_id, existing.bundle_id)
                .await?
                .ok_or_else(|| AppError::Conflict("幂等记录关联的配置包不存在".into()))?;
            if bundle.sha256.as_deref() != Some(package_sha256) {
                return Err(AppError::Conflict(
                    "Idempotency-Key 已用于其他配置包".into(),
                ));
            }
            Ok::<_, AppError>(Some((existing, bundle)))
        }
        .await;
        match result {
            Ok(Some((existing, bundle))) => {
                transaction.commit_audited().await?;
                Ok(Some((existing, bundle)))
            }
            Ok(None) => {
                transaction.rollback().await?;
                Ok(None)
            }
            Err(error) => {
                transaction.rollback().await?;
                Err(error)
            }
        }
    }

    pub async fn create_transfer_from_package(
        &self,
        actor: &ActorContext,
        bundle_id: i64,
        idempotency_key_hash: &str,
    ) -> AppResult<RequestTenantConfigTransferOutcome> {
        validate_sha256(idempotency_key_hash)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let parsed = self.load_bundle_package(tenant_id, bundle_id).await?;
        let request_fingerprint =
            transfer_request_fingerprint(REQUEST_KIND_FROM_PACKAGE, bundle_id);
        let transaction = self.persistence.begin().await?;
        let operation = async {
            let fence = transaction
                .lock_tenant_configuration(tenant_id, None)
                .await?;
            self.product
                .ensure_capability_requirements_in_txn(
                    transaction.product(),
                    tenant_id,
                    &parsed.manifest.required_capabilities,
                )
                .await?;
            if let Some(existing) = transaction
                .find_transfer_by_idempotency_key(tenant_id, actor.user_id, idempotency_key_hash)
                .await?
            {
                ensure_transfer_request_identity(
                    &existing,
                    REQUEST_KIND_FROM_PACKAGE,
                    &request_fingerprint,
                )?;
                if existing.bundle_id != bundle_id {
                    return Err(AppError::Conflict(
                        "Idempotency-Key 已用于其他配置包".into(),
                    ));
                }
                let bundle = transaction
                    .lock_bundle(tenant_id, bundle_id)
                    .await?
                    .ok_or_else(|| AppError::Conflict("幂等记录关联的配置包不存在".into()))?;
                return Ok::<_, AppError>((existing, bundle, false));
            }
            let bundle = transaction
                .lock_bundle(tenant_id, bundle_id)
                .await?
                .ok_or_else(|| AppError::NotFound("配置包不存在".into()))?;
            ensure_bundle_available(&bundle, transaction.database_now().await?)?;
            let transfer = transaction
                .insert_transfer(new_transfer_model(
                    tenant_id,
                    bundle.id,
                    idempotency_key_hash,
                    REQUEST_KIND_FROM_PACKAGE,
                    &request_fingerprint,
                    actor.user_id,
                    fence.configuration_version,
                    fence.authorization_epoch,
                    transaction.database_now().await?,
                )?)
                .await?;
            Ok((transfer, bundle, true))
        }
        .await;
        match operation {
            Ok((transfer, bundle, inserted)) => {
                transaction.commit_audited().await?;
                Ok(RequestTenantConfigTransferOutcome {
                    transfer: TenantConfigTransferVo::from_models(transfer, &bundle)?,
                    inserted,
                })
            }
            Err(error) => {
                transaction.rollback().await?;
                Err(error)
            }
        }
    }

    async fn insert_uploaded_bundle_and_transfer(
        &self,
        actor: &ActorContext,
        file_id: i64,
        parsed: ParsedTenantConfigPackage,
        idempotency_key_hash: &str,
    ) -> AppResult<RequestTenantConfigTransferOutcome> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        let operation = async {
            let fence = transaction
                .lock_tenant_configuration(tenant_id, None)
                .await?;
            self.product
                .ensure_capability_requirements_in_txn(
                    transaction.product(),
                    tenant_id,
                    &parsed.manifest.required_capabilities,
                )
                .await?;
            if let Some(existing) = transaction
                .find_transfer_by_idempotency_key(tenant_id, actor.user_id, idempotency_key_hash)
                .await?
            {
                ensure_transfer_request_identity(
                    &existing,
                    REQUEST_KIND_UPLOAD,
                    &parsed.package_sha256,
                )?;
                let existing_bundle = transaction
                    .lock_bundle(tenant_id, existing.bundle_id)
                    .await?
                    .ok_or_else(|| AppError::Conflict("幂等记录关联的配置包不存在".into()))?;
                if existing_bundle.sha256.as_deref() != Some(parsed.package_sha256.as_str()) {
                    return Err(AppError::Conflict(
                        "Idempotency-Key 已用于其他配置包".into(),
                    ));
                }
                return Ok::<_, AppError>((existing, existing_bundle, false));
            }
            let now = transaction.database_now().await?;
            transaction
                .ensure_config_package_file_ready(tenant_id, file_id, now)
                .await?;
            let bundle_id = next_id()?;
            let counts = serde_json::to_value(&parsed.manifest.resource_counts)
                .map_err(internal_json_error)?;
            let bundle = transaction
                .insert_bundle(TenantConfigBundleRecord {
                    id: bundle_id,
                    tenant_id: tenant_id.to_owned(),
                    origin: TenantConfigBundleRecord::ORIGIN_UPLOADED.to_owned(),
                    source_tenant_key: parsed.manifest.source_tenant_key,
                    source_tenant_name_snapshot: parsed.manifest.source_tenant_name,
                    package_schema_version: parsed.manifest.schema,
                    source_app_version: parsed.manifest.source_app_version,
                    file_id: Some(file_id),
                    sha256: Some(parsed.package_sha256.clone()),
                    resource_counts: counts,
                    item_count: i32::try_from(parsed.manifest.item_count)
                        .map_err(|_| AppError::PayloadTooLarge("配置包项目数量超限".into()))?,
                    status: TenantConfigBundleRecord::STATUS_SUCCEEDED.to_owned(),
                    background_job_id: None,
                    idempotency_key_hash: None,
                    created_by: actor.user_id,
                    error_summary: None,
                    expires_at: Some(now + Duration::hours(i64::from(self.config.artifact_hours))),
                    created_at: now,
                    updated_at: now,
                })
                .await?;
            let transfer = transaction
                .insert_transfer(new_transfer_model(
                    tenant_id,
                    bundle_id,
                    idempotency_key_hash,
                    REQUEST_KIND_UPLOAD,
                    &parsed.package_sha256,
                    actor.user_id,
                    fence.configuration_version,
                    fence.authorization_epoch,
                    now,
                )?)
                .await?;
            Ok((transfer, bundle, true))
        }
        .await;
        match operation {
            Ok((transfer, bundle, inserted)) => {
                transaction.commit_audited().await?;
                Ok(RequestTenantConfigTransferOutcome {
                    transfer: TenantConfigTransferVo::from_models(transfer, &bundle)?,
                    inserted,
                })
            }
            Err(error) => {
                transaction.rollback().await?;
                Err(error)
            }
        }
    }

    pub(super) async fn enqueue_transfer_operation(
        &self,
        actor: &ActorContext,
        transfer_id: i64,
        idempotency_key_hash: &str,
        job_type: &'static str,
        operation: TransferOperationRequest,
    ) -> AppResult<TenantConfigTransferVo> {
        validate_sha256(idempotency_key_hash)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        let result = async {
            transaction
                .lock_tenant_configuration(tenant_id, None)
                .await?;
            let mut transfer = transaction
                .lock_transfer(tenant_id, transfer_id)
                .await?
                .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
            if transfer.requested_by != actor.user_id {
                return Err(AppError::Authorization("只能操作本人创建的配置迁移".into()));
            }
            let now = transaction.database_now().await?;
            let trace = crate::trace_context::current_trace_context();
            let enqueued = self
                .queue
                .enqueue_in_transaction(
                    transaction.background_jobs(),
                    EnqueueJob {
                        tenant_id: Some(tenant_id.to_owned()),
                        schedule_id: None,
                        scheduled_for: Some(now),
                        max_runtime_seconds: Some(self.max_runtime_seconds()?),
                        job_type: job_type.to_owned(),
                        payload: json!({ "transfer_id": transfer_id.to_string() }),
                        priority: -5,
                        available_at: now,
                        max_attempts: MAX_ATTEMPTS,
                        dedupe_key: Some(format!(
                            "{tenant_id}:{}:{transfer_id}:{idempotency_key_hash}",
                            actor.user_id
                        )),
                        traceparent: trace.traceparent,
                        tracestate: trace.tracestate,
                    },
                )
                .await?;
            if !enqueued.inserted {
                if operation_job_id(&transfer, &operation) == Some(enqueued.job_id) {
                    validate_operation_replay_identity(&transfer, &operation)?;
                    let bundle = transaction
                        .lock_bundle(tenant_id, transfer.bundle_id)
                        .await?
                        .ok_or_else(|| AppError::Internal("配置迁移关联的配置包不存在".into()))?;
                    return Ok::<_, AppError>((transfer, bundle));
                }
                return Err(AppError::Conflict("幂等键已被其他配置迁移操作使用".into()));
            }
            validate_operation_request(&transfer, &operation)?;
            clear_superseded_dead_operation_jobs(transaction.as_ref(), &mut transfer, &operation)
                .await?;
            match operation {
                TransferOperationRequest::Preview => {
                    if enqueued.inserted {
                        transfer.status =
                            TenantConfigTransferRecord::STATUS_PREVIEW_PENDING.to_owned();
                        transfer.preview_background_job_id = Some(enqueued.job_id);
                        transfer.preview_calculated_at = None;
                        transfer.plan_hash = None;
                        transfer.error_summary = None;
                    } else if transfer.preview_background_job_id != Some(enqueued.job_id) {
                        return Err(AppError::Conflict("预览幂等键已被其他预览请求使用".into()));
                    }
                }
                TransferOperationRequest::Apply(command) => {
                    if transfer.plan_hash.as_deref() != Some(command.plan_hash.as_str())
                        || transfer.target_configuration_version
                            != command.target_configuration_version
                        || transfer.target_authorization_epoch != command.target_authorization_epoch
                    {
                        return Err(AppError::Conflict("预览结果已失效，请重新预览".into()));
                    }
                    if enqueued.inserted {
                        transfer.status =
                            TenantConfigTransferRecord::STATUS_APPLY_PENDING.to_owned();
                        transfer.apply_background_job_id = Some(enqueued.job_id);
                        transfer.error_summary = None;
                    } else if transfer.apply_background_job_id != Some(enqueued.job_id) {
                        return Err(AppError::Conflict("应用幂等键已被其他请求使用".into()));
                    }
                }
                TransferOperationRequest::Rollback => {
                    if enqueued.inserted {
                        transfer.status =
                            TenantConfigTransferRecord::STATUS_ROLLBACK_PENDING.to_owned();
                        transfer.rollback_background_job_id = Some(enqueued.job_id);
                        transfer.error_summary = None;
                    } else if transfer.rollback_background_job_id != Some(enqueued.job_id) {
                        return Err(AppError::Conflict("回滚幂等键已被其他请求使用".into()));
                    }
                }
            }
            transfer.updated_at = now;
            let transfer = transaction.update_transfer(transfer).await?;
            let bundle = transaction
                .lock_bundle(tenant_id, transfer.bundle_id)
                .await?
                .ok_or_else(|| AppError::Internal("配置迁移关联的配置包不存在".into()))?;
            Ok::<_, AppError>((transfer, bundle))
        }
        .await;
        match result {
            Ok((transfer, bundle)) => {
                transaction.commit_audited().await?;
                self.queue.notify_background_jobs().await;
                TenantConfigTransferVo::from_models(transfer, &bundle)
            }
            Err(error) => {
                transaction.rollback().await?;
                Err(error)
            }
        }
    }

    pub(super) async fn ensure_transfer_visible(
        &self,
        tenant_id: &str,
        transfer_id: i64,
    ) -> AppResult<()> {
        self.persistence
            .find_transfer(tenant_id, transfer_id)
            .await?
            .map(|_| ())
            .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))
    }
}

pub(super) enum TransferOperationRequest {
    Preview,
    Apply(ApplyTenantConfigTransferCommand),
    Rollback,
}

async fn clear_superseded_dead_operation_jobs(
    transaction: &dyn TenantConfigTransferTransaction,
    transfer: &mut TenantConfigTransferRecord,
    operation: &TransferOperationRequest,
) -> AppResult<()> {
    let candidates = match operation {
        TransferOperationRequest::Preview => [
            transfer.apply_background_job_id,
            transfer.rollback_background_job_id,
        ],
        TransferOperationRequest::Apply(_) => [
            transfer.preview_background_job_id,
            transfer.rollback_background_job_id,
        ],
        TransferOperationRequest::Rollback => [
            transfer.preview_background_job_id,
            transfer.apply_background_job_id,
        ],
    }
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(());
    }
    let dead_ids = transaction
        .dead_background_job_ids(&transfer.tenant_id, &candidates)
        .await?;

    // 新操作只废止其他类型的死信执行资格；成功任务指针、应用版本、快照和回滚窗口
    // 均予以保留，因此不会破坏合法回滚链或历史任务关联。
    match operation {
        TransferOperationRequest::Preview => {
            if transfer
                .apply_background_job_id
                .is_some_and(|id| dead_ids.contains(&id))
            {
                transfer.apply_background_job_id = None;
            }
            if transfer
                .rollback_background_job_id
                .is_some_and(|id| dead_ids.contains(&id))
            {
                transfer.rollback_background_job_id = None;
            }
        }
        TransferOperationRequest::Apply(_) => {
            if transfer
                .preview_background_job_id
                .is_some_and(|id| dead_ids.contains(&id))
            {
                transfer.preview_background_job_id = None;
            }
            if transfer
                .rollback_background_job_id
                .is_some_and(|id| dead_ids.contains(&id))
            {
                transfer.rollback_background_job_id = None;
            }
        }
        TransferOperationRequest::Rollback => {
            if transfer
                .preview_background_job_id
                .is_some_and(|id| dead_ids.contains(&id))
            {
                transfer.preview_background_job_id = None;
            }
            if transfer
                .apply_background_job_id
                .is_some_and(|id| dead_ids.contains(&id))
            {
                transfer.apply_background_job_id = None;
            }
        }
    }
    Ok(())
}
