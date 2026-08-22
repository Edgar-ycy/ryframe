use super::*;

impl TenantConfigTransferService {
    pub(super) async fn execute_rollback(&self, job: &ClaimedBackgroundJob) -> AppResult<()> {
        let tenant_id = job_tenant(job)?;
        let transfer_id = payload_id(job, "transfer_id")?;
        let owner_token = Uuid::new_v4().to_string();
        let transfer = self
            .persistence
            .find_transfer(tenant_id, transfer_id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
        if transfer.rollback_background_job_id != Some(job.id) {
            return Ok(());
        }
        if transfer.status == TenantConfigTransferRecord::STATUS_ROLLED_BACK {
            return self.sync_committed_cache_state(tenant_id, &transfer).await;
        }
        let requester = self
            .user_service
            .resolve_current_authorization(
                tenant_id,
                transfer.requested_by,
                TRANSFER_ROLLBACK_PERMISSION,
            )
            .await?;
        let now = self.persistence.database_now().await?;
        if transfer
            .rollback_expires_at
            .is_none_or(|expires_at| expires_at <= now)
        {
            return Err(AppError::Conflict("配置回滚窗口已过期".into()));
        }
        let snapshot_file_id = transfer
            .snapshot_file_id
            .ok_or_else(|| AppError::Conflict("配置回滚快照不存在".into()))?;
        let snapshot_file = self
            .file_service
            .download_config_package_internal(tenant_id, snapshot_file_id)
            .await?;
        let snapshot = parse_tenant_config_package(
            Arc::clone(&self.archive),
            snapshot_file.data,
            self.package_limits(),
        )
        .await?;
        let mut lease = self
            .acquire_operation_lease(
                tenant_id,
                transfer_id,
                &owner_token,
                "tenant_config.rollback",
            )
            .await?;
        if let Err(error) = self
            .mark_transfer_running(
                tenant_id,
                transfer_id,
                job.id,
                TENANT_CONFIG_ROLLBACK_JOB_TYPE,
                Some(&owner_token),
            )
            .await
        {
            let _ = lease.release().await;
            return Err(error);
        }
        let transaction = match self.persistence.begin().await {
            Ok(transaction) => transaction,
            Err(error) => {
                let _ = lease.release().await;
                return Err(error);
            }
        };
        let operation = async {
            let fence = transaction
                .lock_tenant_configuration(tenant_id, Some(&owner_token))
                .await?;
            self.product_service
                .ensure_capability_requirements_in_txn(
                    transaction.product(),
                    tenant_id,
                    &snapshot.manifest.required_capabilities,
                )
                .await?;
            let mut current = transaction
                .lock_transfer(tenant_id, transfer_id)
                .await?
                .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
            if current.rollback_background_job_id != Some(job.id)
                || current.status != TenantConfigTransferRecord::STATUS_ROLLING_BACK
            {
                return Err(AppError::Conflict("配置回滚任务已被替换".into()));
            }
            if Some(fence.configuration_version) != current.applied_configuration_version
                || Some(fence.authorization_epoch) != current.applied_authorization_epoch
            {
                return Err(AppError::Conflict(
                    "应用完成后配置已被修改，不能自动回滚".into(),
                ));
            }
            let rollback_time = transaction.database_now().await?;
            transaction
                .ensure_requester_snapshot(
                    tenant_id,
                    requester_record(&requester),
                    fence,
                    rollback_time,
                )
                .await?;
            transaction
                .ensure_rollback_references_safe(tenant_id, transfer_id)
                .await?;
            transaction
                .restore_snapshot(
                    tenant_id,
                    &snapshot.resources,
                    transfer_id,
                    &self.target_catalog,
                    rollback_time,
                )
                .await?;
            let configuration_version = transaction
                .increment_configuration_version(tenant_id)
                .await?;
            let authorization_epoch = self
                .authorization_cache
                .increment_tenant_epoch_in_transaction(
                    transaction.authorization_mirror(),
                    tenant_id,
                )
                .await?;
            let namespace_version = self
                .authorization_cache
                .record_namespace_version_in_transaction(
                    transaction.authorization_mirror(),
                    tenant_id,
                    CONFIG_CACHE_NAMESPACE,
                )
                .await?;
            current.status = TenantConfigTransferRecord::STATUS_ROLLED_BACK.to_owned();
            current.applied_configuration_version = Some(configuration_version);
            current.applied_authorization_epoch = Some(authorization_epoch);
            current.error_summary = None;
            current.updated_at = transaction.database_now().await?;
            transaction.update_transfer(current).await?;
            transaction
                .mark_plan_outcome(
                    tenant_id,
                    transfer_id,
                    TenantConfigTransferItemRecord::OUTCOME_ROLLED_BACK,
                )
                .await?;
            transaction.release_lease(tenant_id, &owner_token).await?;
            Ok::<_, AppError>((authorization_epoch, namespace_version))
        }
        .await;
        match operation {
            Ok((epoch, namespace_version)) => {
                if let Err(error) = transaction.commit().await {
                    let _ = lease.release().await;
                    return Err(error);
                }
                lease.finish();
                self.authorization_cache
                    .sync_tenant_epoch(tenant_id, epoch)
                    .await?;
                self.authorization_cache
                    .sync_namespace_version(tenant_id, CONFIG_CACHE_NAMESPACE, namespace_version)
                    .await
            }
            Err(error) => {
                let rollback_result = transaction.rollback().await;
                let _ = lease.release().await;
                rollback_result?;
                Err(error)
            }
        }
    }
}
