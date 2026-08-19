use super::*;

fn config_bundle_not_required_by_active_transfer() -> SimpleExpr {
    // 只保护已经入队或正在执行的预览、应用操作；尚未提交和已完成的预览会随源包到期失效。
    // `pending_preview` 是旧版本中无法区分“待用户操作/已入队”的状态，滚动升级期间按活跃状态保守保护。
    Expr::cust(
        "NOT EXISTS (SELECT 1 FROM sys_tenant_config_transfer transfer WHERE transfer.tenant_id = sys_tenant_config_bundle.tenant_id AND transfer.bundle_id = sys_tenant_config_bundle.id AND transfer.status IN ('pending_preview', 'preview_pending', 'previewing', 'apply_pending', 'applying'))",
    )
}

fn config_snapshot_not_used_by_active_rollback() -> SimpleExpr {
    Expr::cust("sys_tenant_config_transfer.status NOT IN ('rollback_pending', 'rolling_back')")
}

impl DataRetentionService {
    pub(super) async fn preview_tenant_config_artifacts(
        &self,
        now: DateTime<Utc>,
    ) -> AppResult<BTreeMap<String, u64>> {
        let packages = tenant_config_bundle::Entity::find()
            .filter(tenant_config_bundle::Column::FileId.is_not_null())
            .filter(tenant_config_bundle::Column::ExpiresAt.lte(now))
            .filter(tenant_config_bundle::Column::Status.is_in([
                tenant_config_bundle::Model::STATUS_SUCCEEDED,
                tenant_config_bundle::Model::STATUS_FAILED,
                tenant_config_bundle::Model::STATUS_EXPIRED,
            ]))
            .filter(config_bundle_not_required_by_active_transfer())
            .count(self.db.write())
            .await
            .map_err(database_error)?;
        let snapshots = tenant_config_transfer::Entity::find()
            .filter(tenant_config_transfer::Column::SnapshotFileId.is_not_null())
            .filter(tenant_config_transfer::Column::RollbackExpiresAt.lte(now))
            .filter(config_snapshot_not_used_by_active_rollback())
            .count(self.db.write())
            .await
            .map_err(database_error)?;
        Ok(BTreeMap::from([
            ("tenant_config_packages".to_owned(), packages),
            ("tenant_config_snapshots".to_owned(), snapshots),
        ]))
    }

    pub(super) async fn cleanup_tenant_config_artifacts(
        &self,
        now: DateTime<Utc>,
    ) -> AppResult<BTreeMap<String, RetentionCleanupResult>> {
        let packages = self.cleanup_expired_config_packages(now).await?;
        let snapshots = self.cleanup_expired_config_snapshots(now).await?;
        Ok(BTreeMap::from([
            ("tenant_config_packages".to_owned(), packages),
            ("tenant_config_snapshots".to_owned(), snapshots),
        ]))
    }

    pub(super) async fn cleanup_expired_config_packages(
        &self,
        before: DateTime<Utc>,
    ) -> AppResult<RetentionCleanupResult> {
        let maximum = self.config.max_rows_per_resource_per_run;
        let mut deleted = 0_u64;
        while usize::try_from(deleted).unwrap_or(usize::MAX) < maximum {
            let remaining_limit =
                maximum.saturating_sub(usize::try_from(deleted).unwrap_or(usize::MAX));
            let limit = self.config.cleanup_batch_size.min(remaining_limit);
            if limit == 0 {
                break;
            }
            let candidates = tenant_config_bundle::Entity::find()
                .filter(tenant_config_bundle::Column::FileId.is_not_null())
                .filter(tenant_config_bundle::Column::ExpiresAt.lte(before))
                .filter(tenant_config_bundle::Column::Status.is_in([
                    tenant_config_bundle::Model::STATUS_SUCCEEDED,
                    tenant_config_bundle::Model::STATUS_FAILED,
                    tenant_config_bundle::Model::STATUS_EXPIRED,
                ]))
                .filter(config_bundle_not_required_by_active_transfer())
                .order_by_asc(tenant_config_bundle::Column::ExpiresAt)
                .order_by_asc(tenant_config_bundle::Column::Id)
                .limit(u64::try_from(limit).unwrap_or(u64::MAX))
                .all(self.db.write())
                .await
                .map_err(database_error)?;
            if candidates.is_empty() {
                break;
            }
            let batch_len = candidates.len();
            for candidate in candidates {
                if self
                    .detach_expired_config_package(candidate, before)
                    .await?
                {
                    deleted = deleted.saturating_add(1);
                }
            }
            if batch_len < limit {
                break;
            }
        }
        let remaining = tenant_config_bundle::Entity::find()
            .filter(tenant_config_bundle::Column::FileId.is_not_null())
            .filter(tenant_config_bundle::Column::ExpiresAt.lte(before))
            .filter(tenant_config_bundle::Column::Status.is_in([
                tenant_config_bundle::Model::STATUS_SUCCEEDED,
                tenant_config_bundle::Model::STATUS_FAILED,
                tenant_config_bundle::Model::STATUS_EXPIRED,
            ]))
            .filter(config_bundle_not_required_by_active_transfer())
            .count(self.db.write())
            .await
            .map_err(database_error)?;
        Ok(RetentionCleanupResult { deleted, remaining })
    }

    pub(super) async fn detach_expired_config_package(
        &self,
        candidate: tenant_config_bundle::Model,
        before: DateTime<Utc>,
    ) -> AppResult<bool> {
        let Some(file_id) = candidate.file_id else {
            return Ok(false);
        };
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        TenantRepository
            .lock_tenant_in_txn(&transaction, &candidate.tenant_id)
            .await?;
        let Some(current) = tenant_config_bundle::Entity::find_by_id(candidate.id)
            .filter(tenant_config_bundle::Column::TenantId.eq(&candidate.tenant_id))
            .filter(tenant_config_bundle::Column::FileId.eq(file_id))
            .filter(tenant_config_bundle::Column::ExpiresAt.lte(before))
            .filter(tenant_config_bundle::Column::Status.is_in([
                tenant_config_bundle::Model::STATUS_SUCCEEDED,
                tenant_config_bundle::Model::STATUS_FAILED,
                tenant_config_bundle::Model::STATUS_EXPIRED,
            ]))
            .filter(config_bundle_not_required_by_active_transfer())
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(database_error)?
        else {
            transaction.rollback().await.map_err(database_error)?;
            return Ok(false);
        };
        let mut active: tenant_config_bundle::ActiveModel = current.into();
        active.file_id = Set(None);
        active.status = Set(tenant_config_bundle::Model::STATUS_EXPIRED.to_owned());
        let now = self.repository.database_utc_now(&transaction).await?;
        active.updated_at = Set(now);
        active.update(&transaction).await.map_err(database_error)?;
        // 同一内容的内部文件可能由多个配置包或快照共享。当前业务引用到期后始终
        // 提交解绑；只有最后一个引用消失时，仓储更新才会把物理文件置为清理墓碑。
        let _marked_for_cleanup = FileRepository
            .mark_unreferenced_config_package_for_cleanup_in_txn(
                &transaction,
                &candidate.tenant_id,
                file_id,
                now,
                now + Duration::minutes(15),
            )
            .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(true)
    }

    pub(super) async fn cleanup_expired_config_snapshots(
        &self,
        before: DateTime<Utc>,
    ) -> AppResult<RetentionCleanupResult> {
        let maximum = self.config.max_rows_per_resource_per_run;
        let mut deleted = 0_u64;
        while usize::try_from(deleted).unwrap_or(usize::MAX) < maximum {
            let remaining_limit =
                maximum.saturating_sub(usize::try_from(deleted).unwrap_or(usize::MAX));
            let limit = self.config.cleanup_batch_size.min(remaining_limit);
            if limit == 0 {
                break;
            }
            let candidates = tenant_config_transfer::Entity::find()
                .filter(tenant_config_transfer::Column::SnapshotFileId.is_not_null())
                .filter(tenant_config_transfer::Column::RollbackExpiresAt.lte(before))
                .filter(config_snapshot_not_used_by_active_rollback())
                .order_by_asc(tenant_config_transfer::Column::RollbackExpiresAt)
                .order_by_asc(tenant_config_transfer::Column::Id)
                .limit(u64::try_from(limit).unwrap_or(u64::MAX))
                .all(self.db.write())
                .await
                .map_err(database_error)?;
            if candidates.is_empty() {
                break;
            }
            let batch_len = candidates.len();
            for candidate in candidates {
                if self
                    .detach_expired_config_snapshot(candidate, before)
                    .await?
                {
                    deleted = deleted.saturating_add(1);
                }
            }
            if batch_len < limit {
                break;
            }
        }
        let remaining = tenant_config_transfer::Entity::find()
            .filter(tenant_config_transfer::Column::SnapshotFileId.is_not_null())
            .filter(tenant_config_transfer::Column::RollbackExpiresAt.lte(before))
            .filter(config_snapshot_not_used_by_active_rollback())
            .count(self.db.write())
            .await
            .map_err(database_error)?;
        Ok(RetentionCleanupResult { deleted, remaining })
    }

    pub(super) async fn detach_expired_config_snapshot(
        &self,
        candidate: tenant_config_transfer::Model,
        before: DateTime<Utc>,
    ) -> AppResult<bool> {
        let Some(file_id) = candidate.snapshot_file_id else {
            return Ok(false);
        };
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        TenantRepository
            .lock_tenant_in_txn(&transaction, &candidate.tenant_id)
            .await?;
        let Some(current) = tenant_config_transfer::Entity::find_by_id(candidate.id)
            .filter(tenant_config_transfer::Column::TenantId.eq(&candidate.tenant_id))
            .filter(tenant_config_transfer::Column::SnapshotFileId.eq(file_id))
            .filter(tenant_config_transfer::Column::RollbackExpiresAt.lte(before))
            .filter(config_snapshot_not_used_by_active_rollback())
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(database_error)?
        else {
            transaction.rollback().await.map_err(database_error)?;
            return Ok(false);
        };
        let mut active: tenant_config_transfer::ActiveModel = current.into();
        active.snapshot_file_id = Set(None);
        let now = self.repository.database_utc_now(&transaction).await?;
        active.updated_at = Set(now);
        active.update(&transaction).await.map_err(database_error)?;
        // 快照也可能与其他记录复用同一文件；先可靠解除到期引用，最后一个引用
        // 的事务负责把文件置为清理墓碑，其他事务不应因此让整轮保留任务失败。
        let _marked_for_cleanup = FileRepository
            .mark_unreferenced_config_package_for_cleanup_in_txn(
                &transaction,
                &candidate.tenant_id,
                file_id,
                now,
                now + Duration::minutes(15),
            )
            .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(true)
    }
}
