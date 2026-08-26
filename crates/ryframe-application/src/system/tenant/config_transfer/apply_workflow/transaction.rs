use super::*;

struct TransactionApplyPlan {
    transfer: TenantConfigTransferRecord,
    plan: PreviewPlan,
    mutation_time: DateTime<Utc>,
}

impl TenantConfigTransferService {
    pub(super) async fn commit_apply_transaction(
        &self,
        prepared: &PreparedApply,
        snapshot_file_id: i64,
    ) -> AppResult<AppliedCacheVersions> {
        let transaction = self.persistence.begin().await?;
        let operation = async {
            let plan = self
                .prepare_transaction_apply(&*transaction, prepared, snapshot_file_id)
                .await?;
            self.apply_transaction_changes(&*transaction, prepared, snapshot_file_id, plan)
                .await
        }
        .await;
        match operation {
            Ok(versions) => {
                transaction
                    .commit(crate::TransactionAuditMode::Skip)
                    .await?;
                Ok(versions)
            }
            Err(error) => {
                transaction.rollback().await?;
                Err(error)
            }
        }
    }

    async fn prepare_transaction_apply(
        &self,
        transaction: &dyn crate::ports::tenant_config::TenantConfigTransferTransaction,
        prepared: &PreparedApply,
        snapshot_file_id: i64,
    ) -> AppResult<TransactionApplyPlan> {
        let fence = transaction
            .lock_tenant_configuration(&prepared.tenant_id, Some(&prepared.owner_token))
            .await?;
        self.product
            .ensure_capability_requirements_in_txn(
                transaction.product(),
                &prepared.tenant_id,
                &prepared.package.manifest.required_capabilities,
            )
            .await?;
        let transfer = transaction
            .lock_transfer(&prepared.tenant_id, prepared.transfer_id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
        ensure_apply_task_current(&transfer, prepared.job_id)?;
        ensure_apply_fence(&transfer, fence)?;
        let mutation_time = transaction.database_now().await?;
        transaction
            .ensure_config_package_file_ready(&prepared.tenant_id, snapshot_file_id, mutation_time)
            .await?;
        transaction
            .ensure_requester_snapshot(
                &prepared.tenant_id,
                requester_record(&prepared.requester),
                fence,
                mutation_time,
            )
            .await?;
        let target = transaction.load_resources(&prepared.tenant_id).await?;
        let plan = build_preview_plan(
            &prepared.tenant_id,
            prepared.transfer_id,
            &prepared.package,
            &target,
            &self.target_catalog.page_routes,
            &self.target_catalog.api_permission_codes,
            fence.configuration_version,
            fence.authorization_epoch,
            mutation_time,
        )?;
        ensure_apply_plan_ready(&transfer, &plan)?;
        transaction
            .ensure_role_quota(&prepared.tenant_id, &plan.items)
            .await?;
        Ok(TransactionApplyPlan {
            transfer,
            plan,
            mutation_time,
        })
    }

    async fn apply_transaction_changes(
        &self,
        transaction: &dyn crate::ports::tenant_config::TenantConfigTransferTransaction,
        prepared: &PreparedApply,
        snapshot_file_id: i64,
        mut apply: TransactionApplyPlan,
    ) -> AppResult<AppliedCacheVersions> {
        transaction
            .apply_resources(
                &prepared.tenant_id,
                &prepared.package.resources,
                &apply.plan.items,
                apply.mutation_time,
            )
            .await?;
        let configuration_version = transaction
            .increment_configuration_version(&prepared.tenant_id)
            .await?;
        let authorization_epoch = self
            .authorization_cache
            .increment_tenant_epoch_in_transaction(
                transaction.authorization_mirror(),
                &prepared.tenant_id,
            )
            .await?;
        let namespace_version = self
            .authorization_cache
            .record_namespace_version_in_transaction(
                transaction.authorization_mirror(),
                &prepared.tenant_id,
                CONFIG_CACHE_NAMESPACE,
            )
            .await?;
        let now = transaction.database_now().await?;
        apply.transfer.status = TenantConfigTransferRecord::STATUS_APPLIED.to_owned();
        apply.transfer.snapshot_file_id = Some(snapshot_file_id);
        apply.transfer.applied_configuration_version = Some(configuration_version);
        apply.transfer.applied_authorization_epoch = Some(authorization_epoch);
        apply.transfer.rollback_expires_at =
            Some(now + Duration::hours(i64::from(self.config.rollback_hours)));
        apply.transfer.error_summary = None;
        apply.transfer.updated_at = now;
        transaction.update_transfer(apply.transfer).await?;
        transaction
            .mark_plan_outcome(
                &prepared.tenant_id,
                prepared.transfer_id,
                TenantConfigTransferItemRecord::OUTCOME_APPLIED,
            )
            .await?;
        transaction
            .release_lease(&prepared.tenant_id, &prepared.owner_token)
            .await?;
        Ok(AppliedCacheVersions {
            authorization_epoch,
            namespace_version,
        })
    }

    pub(super) async fn sync_applied_cache_state(
        &self,
        tenant_id: &str,
        versions: AppliedCacheVersions,
    ) -> AppResult<()> {
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, versions.authorization_epoch)
            .await?;
        self.authorization_cache
            .sync_namespace_version(
                tenant_id,
                CONFIG_CACHE_NAMESPACE,
                versions.namespace_version,
            )
            .await
    }
}

fn ensure_apply_task_current(transfer: &TenantConfigTransferRecord, job_id: i64) -> AppResult<()> {
    if transfer.apply_background_job_id == Some(job_id)
        && transfer.status == TenantConfigTransferRecord::STATUS_APPLYING
    {
        Ok(())
    } else {
        Err(AppError::Conflict("配置应用任务已被替换".into()))
    }
}

fn ensure_apply_fence(
    transfer: &TenantConfigTransferRecord,
    fence: TenantConfigurationFenceRecord,
) -> AppResult<()> {
    if fence.configuration_version == transfer.target_configuration_version
        && fence.authorization_epoch == transfer.target_authorization_epoch
    {
        Ok(())
    } else {
        Err(AppError::Conflict("目标配置已变化，请重新预览".into()))
    }
}

fn ensure_apply_plan_ready(
    transfer: &TenantConfigTransferRecord,
    plan: &PreviewPlan,
) -> AppResult<()> {
    if transfer.plan_hash.as_deref() != Some(plan.plan_hash.as_str()) {
        return Err(AppError::Conflict("预览计划哈希已失效".into()));
    }
    let has_blocking_item = [
        TenantConfigTransferItemRecord::ACTION_BLOCKED,
        TenantConfigTransferItemRecord::ACTION_CONFLICT,
    ]
    .into_iter()
    .any(|action| plan.counts.get(action).copied().unwrap_or(0) > 0);
    if has_blocking_item {
        Err(AppError::Conflict("配置计划仍含冲突或阻断项".into()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_replaced_apply_task() {
        let transfer = applying_transfer();

        assert_conflict(
            ensure_apply_task_current(&transfer, 43),
            "配置应用任务已被替换",
        );
    }

    #[test]
    fn rejects_changed_target_fence() {
        let transfer = applying_transfer();

        assert_conflict(
            ensure_apply_fence(
                &transfer,
                TenantConfigurationFenceRecord {
                    configuration_version: 12,
                    authorization_epoch: 20,
                },
            ),
            "目标配置已变化，请重新预览",
        );
    }

    #[test]
    fn rejects_changed_plan_hash() {
        let transfer = applying_transfer();
        let plan = preview_plan("changed", BTreeMap::new());

        assert_conflict(
            ensure_apply_plan_ready(&transfer, &plan),
            "预览计划哈希已失效",
        );
    }

    #[test]
    fn rejects_blocked_or_conflicting_plan_items() {
        for action in [
            TenantConfigTransferItemRecord::ACTION_BLOCKED,
            TenantConfigTransferItemRecord::ACTION_CONFLICT,
        ] {
            let transfer = applying_transfer();
            let plan = preview_plan("plan", BTreeMap::from([(action.to_owned(), 1)]));

            assert_conflict(
                ensure_apply_plan_ready(&transfer, &plan),
                "配置计划仍含冲突或阻断项",
            );
        }
    }

    #[test]
    fn accepts_current_task_fence_and_conflict_free_plan() {
        let transfer = applying_transfer();
        ensure_apply_task_current(&transfer, 42).expect("当前任务应继续执行");
        ensure_apply_fence(
            &transfer,
            TenantConfigurationFenceRecord {
                configuration_version: 11,
                authorization_epoch: 20,
            },
        )
        .expect("当前目标版本应继续执行");
        ensure_apply_plan_ready(&transfer, &preview_plan("plan", BTreeMap::new()))
            .expect("无冲突计划应继续执行");
    }

    fn preview_plan(plan_hash: &str, counts: BTreeMap<String, u64>) -> PreviewPlan {
        PreviewPlan {
            plan_hash: plan_hash.to_owned(),
            counts,
            items: Vec::new(),
        }
    }

    fn applying_transfer() -> TenantConfigTransferRecord {
        let now = Utc::now();
        TenantConfigTransferRecord {
            id: 1,
            tenant_id: "tenant-a".to_owned(),
            bundle_id: 2,
            idempotency_key_hash: "idempotency".to_owned(),
            request_kind: "upload".to_owned(),
            request_fingerprint: "fingerprint".to_owned(),
            status: TenantConfigTransferRecord::STATUS_APPLYING.to_owned(),
            target_configuration_version: 11,
            target_authorization_epoch: 20,
            plan_hash: Some("plan".to_owned()),
            preview_calculated_at: Some(now),
            preview_background_job_id: Some(40),
            apply_background_job_id: Some(42),
            rollback_background_job_id: None,
            snapshot_file_id: None,
            applied_configuration_version: None,
            applied_authorization_epoch: None,
            change_counts: Value::Null,
            error_summary: None,
            requested_by: 3,
            rollback_expires_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn assert_conflict(result: AppResult<()>, expected: &str) {
        match result {
            Err(AppError::Conflict(message)) => assert_eq!(message, expected),
            other => panic!("期望冲突错误，实际为 {other:?}"),
        }
    }
}
