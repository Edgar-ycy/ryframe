use super::*;

impl TenantConfigTransferService {
    pub(super) async fn execute_rollback(&self, job: &background_job::Model) -> AppResult<()> {
        let tenant_id = job_tenant(job)?;
        let transfer_id = payload_id(job, "transfer_id")?;
        let owner_token = Uuid::new_v4().to_string();
        let transfer = self
            .repository
            .find_transfer_by_id(self.db.write(), tenant_id, transfer_id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
        if transfer.rollback_background_job_id != Some(job.id) {
            return Ok(());
        }
        if transfer.status == tenant_config_transfer::Model::STATUS_ROLLED_BACK {
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
        let now = self.repository.database_utc_now(self.db.write()).await?;
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
        let snapshot =
            parse_tenant_config_package(snapshot_file.data, self.package_limits()).await?;
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
        let transaction = match self.db.write().begin().await.map_err(database_error) {
            Ok(transaction) => transaction,
            Err(error) => {
                let _ = lease.release().await;
                return Err(error);
            }
        };
        let operation = async {
            let fence = self
                .repository
                .lock_tenant_configuration_in_txn(&transaction, tenant_id, Some(&owner_token))
                .await?;
            self.product_service
                .ensure_capability_requirements_in_txn(
                    &transaction,
                    tenant_id,
                    &snapshot.manifest.required_capabilities,
                )
                .await?;
            let mut current = self
                .repository
                .lock_transfer_in_txn(&transaction, tenant_id, transfer_id)
                .await?
                .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
            if current.rollback_background_job_id != Some(job.id)
                || current.status != tenant_config_transfer::Model::STATUS_ROLLING_BACK
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
            let rollback_time = self.repository.database_utc_now(&transaction).await?;
            ensure_requester_snapshot_in_txn(
                &transaction,
                tenant_id,
                &requester,
                fence,
                rollback_time,
            )
            .await?;
            ensure_rollback_references_safe(&transaction, tenant_id, transfer_id).await?;
            restore_snapshot_in_transaction(
                &transaction,
                tenant_id,
                &snapshot.resources,
                transfer_id,
                &self.target_catalog,
                rollback_time,
            )
            .await?;
            let configuration_version = self
                .repository
                .increment_configuration_version_in_txn(&transaction, tenant_id)
                .await?;
            let authorization_epoch = self
                .authorization_cache
                .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
                .await?;
            let namespace_version = self
                .authorization_cache
                .record_namespace_version_in_transaction(
                    &transaction,
                    tenant_id,
                    CONFIG_CACHE_NAMESPACE,
                )
                .await?;
            current.status = tenant_config_transfer::Model::STATUS_ROLLED_BACK.to_owned();
            current.applied_configuration_version = Some(configuration_version);
            current.applied_authorization_epoch = Some(authorization_epoch);
            current.error_summary = None;
            current.updated_at = self.repository.database_utc_now(&transaction).await?;
            self.repository
                .update_transfer(&transaction, current)
                .await?;
            mark_plan_outcome(
                &transaction,
                tenant_id,
                transfer_id,
                tenant_config_transfer_item::Model::OUTCOME_ROLLED_BACK,
            )
            .await?;
            self.repository
                .release_lease_in_txn(&transaction, tenant_id, &owner_token)
                .await?;
            Ok::<_, AppError>((authorization_epoch, namespace_version))
        }
        .await;
        match operation {
            Ok((epoch, namespace_version)) => {
                if let Err(error) = transaction.commit().await.map_err(database_error) {
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
                let rollback_result = transaction.rollback().await.map_err(database_error);
                let _ = lease.release().await;
                rollback_result?;
                Err(error)
            }
        }
    }
}
