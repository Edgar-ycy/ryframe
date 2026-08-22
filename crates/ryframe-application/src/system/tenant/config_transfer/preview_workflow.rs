use super::*;

impl TenantConfigTransferService {
    pub(super) async fn execute_preview(&self, job: &ClaimedBackgroundJob) -> AppResult<()> {
        let tenant_id = job_tenant(job)?;
        let transfer_id = payload_id(job, "transfer_id")?;
        let transfer = self
            .persistence
            .find_transfer(tenant_id, transfer_id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
        if transfer.preview_background_job_id != Some(job.id) {
            return Ok(());
        }
        let requester = self
            .user
            .resolve_current_authorization(
                tenant_id,
                transfer.requested_by,
                TRANSFER_PREVIEW_PERMISSION,
            )
            .await?;
        self.mark_transfer_running(
            tenant_id,
            transfer_id,
            job.id,
            TENANT_CONFIG_PREVIEW_JOB_TYPE,
            None,
        )
        .await?;
        let parsed = self
            .load_bundle_package(tenant_id, transfer.bundle_id)
            .await?;
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
            let mut current = transaction
                .lock_transfer(tenant_id, transfer_id)
                .await?
                .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
            if current.preview_background_job_id != Some(job.id)
                || current.status != TenantConfigTransferRecord::STATUS_PREVIEWING
            {
                return Ok::<_, AppError>(None);
            }
            let calculated_at = transaction.database_now().await?;
            transaction
                .ensure_requester_snapshot(
                    tenant_id,
                    requester_record(&requester),
                    fence,
                    calculated_at,
                )
                .await?;
            let target = transaction.load_resources(tenant_id).await?;
            let plan = build_preview_plan(
                tenant_id,
                transfer_id,
                &parsed,
                &target,
                &self.target_catalog.page_routes,
                &self.target_catalog.api_permission_codes,
                fence.configuration_version,
                fence.authorization_epoch,
                calculated_at,
            )?;
            transaction
                .replace_items(tenant_id, transfer_id, plan.items)
                .await?;
            current.status = TenantConfigTransferRecord::STATUS_PREVIEWED.to_owned();
            current.target_configuration_version = fence.configuration_version;
            current.target_authorization_epoch = fence.authorization_epoch;
            current.plan_hash = Some(plan.plan_hash);
            current.preview_calculated_at = Some(calculated_at);
            current.change_counts =
                serde_json::to_value(plan.counts).map_err(internal_json_error)?;
            current.error_summary = None;
            current.updated_at = calculated_at;
            Ok(Some(transaction.update_transfer(current).await?))
        }
        .await;
        match operation {
            Ok(_) => transaction.commit().await,
            Err(error) => {
                transaction.rollback().await?;
                Err(error)
            }
        }
    }
}
