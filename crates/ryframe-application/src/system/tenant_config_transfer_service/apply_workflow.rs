use super::*;

impl TenantConfigTransferService {
    pub(super) async fn execute_apply(&self, job: &ClaimedBackgroundJob) -> AppResult<()> {
        let tenant_id = job_tenant(job)?;
        let transfer_id = payload_id(job, "transfer_id")?;
        let owner_token = Uuid::new_v4().to_string();
        let transfer = self
            .repository
            .find_transfer_by_id(self.db.write(), tenant_id, transfer_id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
        if transfer.apply_background_job_id != Some(job.id) {
            return Ok(());
        }
        if transfer.status == tenant_config_transfer::Model::STATUS_APPLIED {
            return self.sync_committed_cache_state(tenant_id, &transfer).await;
        }
        let requester = self
            .user_service
            .resolve_current_authorization(
                tenant_id,
                transfer.requested_by,
                TRANSFER_APPLY_PERMISSION,
            )
            .await?;
        let mut lease = self
            .acquire_operation_lease(tenant_id, transfer_id, &owner_token, "tenant_config.apply")
            .await?;
        if let Err(error) = self
            .mark_transfer_running(
                tenant_id,
                transfer_id,
                job.id,
                TENANT_CONFIG_APPLY_JOB_TYPE,
                Some(&owner_token),
            )
            .await
        {
            let _ = lease.release().await;
            return Err(error);
        }
        let parsed = match self
            .load_bundle_package(tenant_id, transfer.bundle_id)
            .await
        {
            Ok(parsed) => parsed,
            Err(error) => {
                let _ = lease.release().await;
                return Err(error);
            }
        };
        // 快照也必须在租约持有者的租户行锁下从同一事务读取。
        let snapshot_transaction = match self.db.write().begin().await.map_err(database_error) {
            Ok(transaction) => transaction,
            Err(error) => {
                let _ = lease.release().await;
                return Err(error);
            }
        };
        let snapshot_result = async {
            let fence = self
                .repository
                .lock_tenant_configuration_in_txn(
                    &snapshot_transaction,
                    tenant_id,
                    Some(&owner_token),
                )
                .await?;
            self.product_service
                .ensure_capability_requirements_in_txn(
                    &snapshot_transaction,
                    tenant_id,
                    &parsed.manifest.required_capabilities,
                )
                .await?;
            let snapshot_time = self
                .repository
                .database_utc_now(&snapshot_transaction)
                .await?;
            ensure_requester_snapshot_in_txn(
                &snapshot_transaction,
                tenant_id,
                &requester,
                fence,
                snapshot_time,
            )
            .await?;
            let tenant = tenant::Entity::find()
                .filter(tenant::Column::TenantId.eq(tenant_id))
                .one(&snapshot_transaction)
                .await
                .map_err(database_error)?
                .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
            let target_resources = load_resources_on(&snapshot_transaction, tenant_id).await?;
            ensure_preview_identity(&transfer, &parsed, &target_resources, fence)?;
            let enabled_capabilities = self
                .product_service
                .enabled_capability_requirements_in_txn(&snapshot_transaction, tenant_id)
                .await?;
            let (snapshot_resources, snapshot_capabilities) = filter_exportable_resources(
                target_resources,
                &self.target_catalog,
                &enabled_capabilities,
            )?;
            Ok::<_, AppError>((
                snapshot_resources,
                snapshot_capabilities,
                tenant.name,
                snapshot_time,
            ))
        }
        .await;
        let (snapshot_resources, snapshot_capabilities, snapshot_tenant_name, snapshot_time) =
            match snapshot_result {
                Ok(value) => {
                    if let Err(error) = snapshot_transaction.commit().await.map_err(database_error)
                    {
                        let _ = lease.release().await;
                        return Err(error);
                    }
                    value
                }
                Err(error) => {
                    let rollback_result = snapshot_transaction
                        .rollback()
                        .await
                        .map_err(database_error);
                    let _ = lease.release().await;
                    rollback_result?;
                    return Err(error);
                }
            };
        let snapshot = match crate::system::build_tenant_config_package(
            Arc::clone(&self.archive),
            snapshot_resources,
            snapshot_capabilities,
            TenantConfigPackageSource {
                tenant_key: tenant_id.to_owned(),
                tenant_name: snapshot_tenant_name,
                app_version: env!("CARGO_PKG_VERSION").to_owned(),
                generated_at: snapshot_time,
            },
            self.package_limits(),
        )
        .await
        {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let _ = lease.release().await;
                return Err(error);
            }
        };
        let snapshot_upload = match self
            .file_service
            .upload_config_package_unbound(
                tenant_id,
                "config-transfer-worker",
                format!("rollback-{transfer_id}.ryframe-config.zip"),
                snapshot.data,
                u64::try_from(self.config.max_package_bytes).unwrap_or(u64::MAX),
            )
            .await
        {
            Ok(uploaded) => uploaded,
            Err(error) => {
                let _ = lease.release().await;
                return Err(error);
            }
        };
        let snapshot_file_id = match parse_file_id(&snapshot_upload.file_id) {
            Ok(file_id) => file_id,
            Err(error) => {
                if let Ok(file_id) = snapshot_upload.file_id.parse::<i64>() {
                    let _ = self
                        .file_service
                        .schedule_unreferenced_config_package_cleanup(tenant_id, file_id)
                        .await;
                }
                let _ = lease.release().await;
                return Err(error);
            }
        };

        if let Err(error) = self.renew_operation_lease(tenant_id, &owner_token).await {
            let _ = self
                .file_service
                .schedule_unreferenced_config_package_cleanup(tenant_id, snapshot_file_id)
                .await;
            let _ = lease.release().await;
            return Err(error);
        }

        let transaction = match self.db.write().begin().await.map_err(database_error) {
            Ok(transaction) => transaction,
            Err(error) => {
                let _ = self
                    .file_service
                    .schedule_unreferenced_config_package_cleanup(tenant_id, snapshot_file_id)
                    .await;
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
                    &parsed.manifest.required_capabilities,
                )
                .await?;
            let mut current = self
                .repository
                .lock_transfer_in_txn(&transaction, tenant_id, transfer_id)
                .await?
                .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
            if current.apply_background_job_id != Some(job.id)
                || current.status != tenant_config_transfer::Model::STATUS_APPLYING
            {
                return Err(AppError::Conflict("配置应用任务已被替换".into()));
            }
            if fence.configuration_version != current.target_configuration_version
                || fence.authorization_epoch != current.target_authorization_epoch
            {
                return Err(AppError::Conflict("目标配置已变化，请重新预览".into()));
            }
            let mutation_time = self.repository.database_utc_now(&transaction).await?;
            ensure_config_package_file_ready_in_txn(
                &transaction,
                tenant_id,
                snapshot_file_id,
                mutation_time,
            )
            .await?;
            ensure_requester_snapshot_in_txn(
                &transaction,
                tenant_id,
                &requester,
                fence,
                mutation_time,
            )
            .await?;
            let target = load_resources_on(&transaction, tenant_id).await?;
            let plan = build_preview_plan(
                tenant_id,
                transfer_id,
                &parsed,
                &target,
                &self.target_catalog.page_routes,
                &self.target_catalog.api_permission_codes,
                fence.configuration_version,
                fence.authorization_epoch,
                mutation_time,
            )?;
            if current.plan_hash.as_deref() != Some(plan.plan_hash.as_str()) {
                return Err(AppError::Conflict("预览计划哈希已失效".into()));
            }
            if plan
                .counts
                .get(tenant_config_transfer_item::Model::ACTION_BLOCKED)
                .copied()
                .unwrap_or(0)
                > 0
                || plan
                    .counts
                    .get(tenant_config_transfer_item::Model::ACTION_CONFLICT)
                    .copied()
                    .unwrap_or(0)
                    > 0
            {
                return Err(AppError::Conflict("配置计划仍含冲突或阻断项".into()));
            }
            ensure_role_quota_for_plan_in_txn(&transaction, tenant_id, &plan.items).await?;
            apply_resources_in_transaction(
                &transaction,
                tenant_id,
                &parsed.resources,
                &plan.items,
                mutation_time,
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
            let now = self.repository.database_utc_now(&transaction).await?;
            current.status = tenant_config_transfer::Model::STATUS_APPLIED.to_owned();
            current.snapshot_file_id = Some(snapshot_file_id);
            current.applied_configuration_version = Some(configuration_version);
            current.applied_authorization_epoch = Some(authorization_epoch);
            current.rollback_expires_at =
                Some(now + Duration::hours(i64::from(self.config.rollback_hours)));
            current.error_summary = None;
            current.updated_at = now;
            self.repository
                .update_transfer(&transaction, current)
                .await?;
            mark_plan_outcome(
                &transaction,
                tenant_id,
                transfer_id,
                tenant_config_transfer_item::Model::OUTCOME_APPLIED,
            )
            .await?;
            self.repository
                .release_lease_in_txn(&transaction, tenant_id, &owner_token)
                .await?;
            Ok::<_, AppError>((authorization_epoch, namespace_version))
        }
        .await;
        match operation {
            Ok((authorization_epoch, namespace_version)) => {
                if let Err(error) = transaction.commit().await.map_err(database_error) {
                    let _ = self
                        .file_service
                        .schedule_unreferenced_config_package_cleanup(tenant_id, snapshot_file_id)
                        .await;
                    let _ = lease.release().await;
                    return Err(error);
                }
                lease.finish();
                self.authorization_cache
                    .sync_tenant_epoch(tenant_id, authorization_epoch)
                    .await?;
                self.authorization_cache
                    .sync_namespace_version(tenant_id, CONFIG_CACHE_NAMESPACE, namespace_version)
                    .await
            }
            Err(error) => {
                let rollback_result = transaction.rollback().await.map_err(database_error);
                let _ = self
                    .file_service
                    .schedule_unreferenced_config_package_cleanup(tenant_id, snapshot_file_id)
                    .await;
                let _ = lease.release().await;
                rollback_result?;
                Err(error)
            }
        }
    }
}
