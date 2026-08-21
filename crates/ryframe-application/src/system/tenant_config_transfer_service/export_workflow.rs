use super::*;

impl TenantConfigTransferService {
    pub(super) async fn execute_export(&self, job: &ClaimedBackgroundJob) -> AppResult<()> {
        let tenant_id = job_tenant(job)?;
        let bundle_id = payload_id(job, "bundle_id")?;
        let requester = self
            .user_service
            .resolve_current_authorization(
                tenant_id,
                self.bundle_requester(tenant_id, bundle_id).await?,
                PACKAGE_EXPORT_PERMISSION,
            )
            .await?;
        let transaction = self.persistence.begin().await?;
        transaction
            .lock_tenant_configuration(tenant_id, None)
            .await?;
        let mut bundle = transaction
            .lock_bundle(tenant_id, bundle_id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置包导出记录不存在".into()))?;
        if bundle.background_job_id != Some(job.id) {
            transaction.rollback().await?;
            return Err(AppError::Conflict("配置包导出任务身份不匹配".into()));
        }
        if bundle.status == TenantConfigBundleRecord::STATUS_SUCCEEDED {
            transaction.rollback().await?;
            return Ok(());
        }
        bundle.status = TenantConfigBundleRecord::STATUS_RUNNING.to_owned();
        bundle.updated_at = transaction.database_now().await?;
        transaction.update_bundle(bundle).await?;
        transaction.commit().await?;

        // 在租户配置行锁保护下从同一事务读取全部资源，避免包内混合两个版本。
        let source_transaction = self.persistence.begin().await?;
        let source_result = async {
            let fence = source_transaction
                .lock_tenant_configuration(tenant_id, None)
                .await?;
            let generated_at = source_transaction.database_now().await?;
            source_transaction
                .ensure_requester_snapshot(
                    tenant_id,
                    requester_record(&requester),
                    fence,
                    generated_at,
                )
                .await?;
            let tenant_name = source_transaction.tenant_name(tenant_id).await?;
            let resources = source_transaction.load_resources(tenant_id).await?;
            let enabled_capabilities = self
                .product_service
                .enabled_capability_requirements_in_txn(source_transaction.product(), tenant_id)
                .await?;
            let (resources, required_capabilities) = filter_exportable_resources(
                resources,
                &self.target_catalog,
                &enabled_capabilities,
            )?;
            Ok::<_, AppError>((resources, required_capabilities, tenant_name, generated_at))
        }
        .await;
        let (source_resources, required_capabilities, source_tenant_name, generated_at) =
            match source_result {
                Ok(source) => {
                    source_transaction.commit().await?;
                    source
                }
                Err(error) => {
                    source_transaction.rollback().await?;
                    return Err(error);
                }
            };
        let generated = crate::system::build_tenant_config_package(
            Arc::clone(&self.archive),
            source_resources,
            required_capabilities,
            TenantConfigPackageSource {
                tenant_key: tenant_id.to_owned(),
                tenant_name: source_tenant_name,
                app_version: env!("CARGO_PKG_VERSION").to_owned(),
                generated_at,
            },
            self.package_limits(),
        )
        .await?;
        let uploaded = self
            .file_service
            .upload_config_package_unbound(
                tenant_id,
                "config-transfer-worker",
                format!(
                    "{}-{}.ryframe-config.zip",
                    tenant_id,
                    generated_at.format("%Y%m%d%H%M%S")
                ),
                generated.data,
                u64::try_from(self.config.max_package_bytes).unwrap_or(u64::MAX),
            )
            .await?;
        let file_id = parse_file_id(&uploaded.file_id)?;
        let final_requester = self
            .user_service
            .resolve_current_authorization(
                tenant_id,
                requester.actor.user_id,
                PACKAGE_EXPORT_PERMISSION,
            )
            .await?;
        let transaction = self.persistence.begin().await?;
        let operation = async {
            let fence = transaction
                .lock_tenant_configuration(tenant_id, None)
                .await?;
            let mut bundle = transaction
                .lock_bundle(tenant_id, bundle_id)
                .await?
                .ok_or_else(|| AppError::NotFound("配置包导出记录不存在".into()))?;
            if bundle.background_job_id != Some(job.id) {
                return Err(AppError::Conflict("配置包导出任务已被替换".into()));
            }
            let now = transaction.database_now().await?;
            transaction
                .ensure_config_package_file_ready(tenant_id, file_id, now)
                .await?;
            transaction
                .ensure_requester_snapshot(
                    tenant_id,
                    requester_record(&final_requester),
                    fence,
                    now,
                )
                .await?;
            bundle.file_id = Some(file_id);
            bundle.sha256 = Some(generated.package_sha256);
            bundle.source_tenant_key = generated.manifest.source_tenant_key;
            bundle.source_tenant_name_snapshot = generated.manifest.source_tenant_name;
            bundle.package_schema_version = generated.manifest.schema;
            bundle.source_app_version = generated.manifest.source_app_version;
            bundle.resource_counts = serde_json::to_value(generated.manifest.resource_counts)
                .map_err(internal_json_error)?;
            bundle.item_count = i32::try_from(generated.manifest.item_count)
                .map_err(|_| AppError::PayloadTooLarge("配置包项目数量超限".into()))?;
            bundle.status = TenantConfigBundleRecord::STATUS_SUCCEEDED.to_owned();
            bundle.error_summary = None;
            bundle.expires_at = Some(now + Duration::hours(i64::from(self.config.artifact_hours)));
            bundle.updated_at = now;
            transaction.update_bundle(bundle).await
        }
        .await;
        match operation {
            Ok(_) => {
                if let Err(error) = transaction.commit().await {
                    // COMMIT 响应丢失时结果可能已经持久化。引用保护会在已绑定成功时拒绝
                    // 清理，而在事务确实未提交时把孤儿文件纳入延迟回收。
                    let _ = self
                        .file_service
                        .schedule_unreferenced_config_package_cleanup(tenant_id, file_id)
                        .await;
                    return Err(error);
                }
                Ok(())
            }
            Err(error) => {
                transaction.rollback().await?;
                let _ = self
                    .file_service
                    .schedule_unreferenced_config_package_cleanup(tenant_id, file_id)
                    .await;
                Err(error)
            }
        }
    }
}
