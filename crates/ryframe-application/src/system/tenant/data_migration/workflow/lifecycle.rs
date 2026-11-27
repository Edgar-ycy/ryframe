use ryframe_kernel::{AppError, AppResult};
use time::{Duration, OffsetDateTime};

use crate::ports::tenant_data::{TenantDataMigrationRecord, TenantDataPlacementRecord};

use super::{TenantDataMigrationService, ensure_not_cancel_requested};

impl TenantDataMigrationService {
    pub(super) async fn enter_maintenance(
        &self,
        snapshot: TenantDataMigrationRecord,
    ) -> AppResult<TenantDataMigrationRecord> {
        let now = self.persistence.database_now().await?;
        let transaction = self.persistence.begin().await?;
        self.acquire_or_renew_operation_lease(&*transaction, &snapshot, now)
            .await?;
        let tenant = transaction
            .lock_tenant(&snapshot.tenant_id, Some(&snapshot.switch_token))
            .await?;
        let mut placement = transaction.lock_placement(&snapshot.tenant_id).await?;
        let mut migration = transaction.lock_migration(snapshot.id).await?;
        ensure_not_cancel_requested(&migration)?;
        if migration.state == TenantDataMigrationRecord::STATE_QUIESCING {
            transaction
                .commit(crate::TransactionAuditMode::Skip)
                .await?;
            return Ok(migration);
        }
        if migration.state != TenantDataMigrationRecord::STATE_QUEUED {
            return Err(AppError::TenantOperationConflict(
                "迁移状态不允许进入 quiescing".into(),
            ));
        }
        if placement.current_target_key != migration.source_target_key
            || placement.placement_generation != migration.source_generation
            || placement.switch_token != migration.source_switch_token
        {
            return Err(AppError::StalePlacementGeneration(
                "源 placement 与迁移计划不一致".into(),
            ));
        }
        let changed = placement.state == TenantDataPlacementRecord::STATE_ACTIVE;
        if changed {
            placement.state = TenantDataPlacementRecord::STATE_MAINTENANCE.into();
            placement.updated_at = now;
            transaction.save_placement(placement).await?;
            transaction
                .increment_runtime_epoch(&migration.tenant_id)
                .await?;
        } else if placement.state != TenantDataPlacementRecord::STATE_MAINTENANCE {
            return Err(AppError::TenantOperationConflict(
                "placement 状态不允许进入维护".into(),
            ));
        }
        migration.state = TenantDataMigrationRecord::STATE_QUIESCING.into();
        migration.quiesced_at.get_or_insert(now);
        migration.updated_at = now;
        migration = transaction.save_migration(migration).await?;
        transaction
            .commit(crate::TransactionAuditMode::Skip)
            .await?;
        if changed {
            self.authorization_cache
                .publish_tenant_context_changed(&migration.tenant_id, tenant.authorization_epoch)
                .await;
        }
        Ok(migration)
    }

    pub(super) async fn cut_over(
        &self,
        snapshot: TenantDataMigrationRecord,
    ) -> AppResult<TenantDataMigrationRecord> {
        // CUTTING_OVER is durable and may be resumed after a crash. Re-establish the
        // complete cross-database safety proof immediately before changing the control
        // pointer; no control transaction is held across these target-database awaits.
        self.targets.verify_now(&snapshot.source_target_key).await?;
        self.targets.verify_now(&snapshot.target_key).await?;
        self.assert_migration_frozen_fence(&snapshot, &snapshot.source_target_key)
            .await?;
        self.assert_migration_frozen_fence(&snapshot, &snapshot.target_key)
            .await?;
        let now = self.persistence.database_now().await?;
        let transaction = self.persistence.begin().await?;
        Self::renew_operation_lease(&*transaction, &snapshot, now).await?;
        let tenant = transaction
            .lock_tenant(&snapshot.tenant_id, Some(&snapshot.switch_token))
            .await?;
        let mut placement = transaction.lock_placement(&snapshot.tenant_id).await?;
        let mut migration = transaction.lock_migration(snapshot.id).await?;
        ensure_not_cancel_requested(&migration)?;
        if migration.state == TenantDataMigrationRecord::STATE_ACTIVATING {
            transaction
                .commit(crate::TransactionAuditMode::Skip)
                .await?;
            return Ok(migration);
        }
        if migration.state != TenantDataMigrationRecord::STATE_CUTTING_OVER {
            return Err(AppError::TenantOperationConflict(
                "迁移状态不允许执行 cutover".into(),
            ));
        }
        if placement.state != TenantDataPlacementRecord::STATE_MAINTENANCE
            || placement.current_target_key != migration.source_target_key
            || placement.placement_generation != migration.source_generation
        {
            return Err(AppError::StalePlacementGeneration(
                "cutover 前 placement 已变化".into(),
            ));
        }
        placement.current_target_key = migration.target_key.clone();
        placement.placement_generation = migration.target_generation;
        placement.switch_token = migration.switch_token.clone();
        placement.updated_at = now;
        transaction.save_placement(placement).await?;
        transaction
            .record_data_scope_change(&migration.tenant_id)
            .await?;
        transaction
            .increment_runtime_epoch(&migration.tenant_id)
            .await?;
        migration.state = TenantDataMigrationRecord::STATE_ACTIVATING.into();
        migration.cut_over_at.get_or_insert(now);
        migration.updated_at = now;
        migration = transaction.save_migration(migration).await?;
        transaction
            .commit(crate::TransactionAuditMode::Skip)
            .await?;
        self.authorization_cache
            .publish_tenant_context_changed(&migration.tenant_id, tenant.authorization_epoch)
            .await;
        Ok(migration)
    }

    pub(super) async fn enter_retention(
        &self,
        snapshot: TenantDataMigrationRecord,
    ) -> AppResult<TenantDataMigrationRecord> {
        let now = self.persistence.database_now().await?;
        let transaction = self.persistence.begin().await?;
        Self::renew_operation_lease(&*transaction, &snapshot, now).await?;
        let mut migration = transaction.lock_migration(snapshot.id).await?;
        if migration.state == TenantDataMigrationRecord::STATE_RETENTION_PENDING {
            transaction
                .commit(crate::TransactionAuditMode::Skip)
                .await?;
            return Ok(migration);
        }
        if migration.state != TenantDataMigrationRecord::STATE_SUCCEEDED {
            return Err(AppError::TenantOperationConflict(
                "迁移状态不允许进入 retention_pending".into(),
            ));
        }
        migration.state = TenantDataMigrationRecord::STATE_RETENTION_PENDING.into();
        migration.retention_until = Some(
            migration
                .succeeded_at
                .unwrap_or(now)
                .checked_add(Duration::hours(i64::from(migration.retention_hours)))
                .ok_or_else(|| AppError::Database("迁移保留截止时间无法精确表示为 UTC".into()))?,
        );
        migration.updated_at = now;
        migration = transaction.save_migration(migration).await?;
        transaction
            .release_lease(&migration.tenant_id, &migration.switch_token)
            .await?;
        transaction
            .commit(crate::TransactionAuditMode::Skip)
            .await?;
        Ok(migration)
    }

    pub(super) async fn set_state<F>(
        &self,
        snapshot: TenantDataMigrationRecord,
        state: &str,
        update: F,
    ) -> AppResult<TenantDataMigrationRecord>
    where
        F: FnOnce(&mut TenantDataMigrationRecord, OffsetDateTime),
    {
        let now = self.persistence.database_now().await?;
        let transaction = self.persistence.begin().await?;
        Self::renew_operation_lease(&*transaction, &snapshot, now).await?;
        let mut migration = transaction.lock_migration(snapshot.id).await?;
        ensure_not_cancel_requested(&migration)?;
        if migration.state == state {
            transaction
                .commit(crate::TransactionAuditMode::Skip)
                .await?;
            return Ok(migration);
        }
        if migration.state != snapshot.state {
            return Err(AppError::TenantOperationConflict(format!(
                "迁移状态已变化: expected={}, actual={}",
                snapshot.state, migration.state
            )));
        }
        migration.state = state.into();
        migration.updated_at = now;
        update(&mut migration, now);
        migration = transaction.save_migration(migration).await?;
        transaction
            .commit(crate::TransactionAuditMode::Skip)
            .await?;
        Ok(migration)
    }
}

pub(super) async fn activate_control(
    persistence: &dyn crate::ports::tenant_data::TenantDataMigrationPersistencePort,
    target: &dyn crate::ports::tenant_data::TenantDataMigrationPort,
    publisher: &dyn crate::DataScopeProjectionPublisher,
    cache: &crate::AuthorizationCache,
    snapshot: TenantDataMigrationRecord,
) -> AppResult<TenantDataMigrationRecord> {
    let fence = crate::ports::tenant_data::TenantDataFence {
        tenant_id: &snapshot.tenant_id,
        target_key: &snapshot.target_key,
        generation: super::checked_generation(snapshot.target_generation, "目标")?,
        switch_token: &snapshot.switch_token,
    };
    target.activate_fence(fence).await?;
    let published_version = publisher.publish_for_migration(fence).await?;
    let now = persistence.database_now().await?;
    let transaction = persistence.begin().await?;
    TenantDataMigrationService::renew_operation_lease(&*transaction, &snapshot, now).await?;
    activate(transaction, &snapshot, published_version, now, cache).await
}

async fn activate(
    transaction: Box<dyn crate::ports::tenant_data::TenantDataMigrationTransaction>,
    snapshot: &TenantDataMigrationRecord,
    published_version: i64,
    now: OffsetDateTime,
    authorization_cache: &crate::AuthorizationCache,
) -> AppResult<TenantDataMigrationRecord> {
    let tenant = transaction
        .lock_tenant(&snapshot.tenant_id, Some(&snapshot.switch_token))
        .await?;
    let mut placement = transaction.lock_placement(&snapshot.tenant_id).await?;
    let mut migration = transaction.lock_migration(snapshot.id).await?;
    ensure_not_cancel_requested(&migration)?;
    ensure_activation_identity(&placement, &migration, snapshot)?;
    if migration.state == TenantDataMigrationRecord::STATE_SUCCEEDED {
        transaction
            .commit(crate::TransactionAuditMode::Skip)
            .await?;
        return Ok(migration);
    }
    if migration.state != TenantDataMigrationRecord::STATE_ACTIVATING {
        return Err(AppError::TenantOperationConflict(
            "迁移状态不允许激活 placement".into(),
        ));
    }
    if tenant.data_scope_version != published_version {
        return Err(AppError::Conflict(
            "激活前数据范围版本已变化，请重新发布投影后重试".into(),
        ));
    }
    if placement.state != TenantDataPlacementRecord::STATE_MAINTENANCE {
        return Err(AppError::TenantOperationConflict(
            "激活前 placement 必须保持维护状态".into(),
        ));
    }
    placement.state = TenantDataPlacementRecord::STATE_ACTIVE.into();
    placement.updated_at = now;
    transaction.save_placement(placement).await?;
    transaction
        .increment_runtime_epoch(&migration.tenant_id)
        .await?;
    let epoch = transaction
        .increment_authorization_epoch(&migration.tenant_id)
        .await?;
    migration.state = TenantDataMigrationRecord::STATE_SUCCEEDED.into();
    migration.activated_at.get_or_insert(now);
    migration.succeeded_at.get_or_insert(now);
    migration.updated_at = now;
    migration = transaction.save_migration(migration).await?;
    transaction
        .commit(crate::TransactionAuditMode::Skip)
        .await?;
    authorization_cache
        .sync_tenant_epoch(&migration.tenant_id, epoch)
        .await?;
    Ok(migration)
}

fn ensure_activation_identity(
    placement: &TenantDataPlacementRecord,
    migration: &TenantDataMigrationRecord,
    snapshot: &TenantDataMigrationRecord,
) -> AppResult<()> {
    if migration.tenant_id != snapshot.tenant_id
        || migration.target_key != snapshot.target_key
        || migration.target_generation != snapshot.target_generation
        || migration.switch_token != snapshot.switch_token
        || placement.tenant_id != snapshot.tenant_id
        || placement.current_target_key != snapshot.target_key
        || placement.placement_generation != snapshot.target_generation
        || placement.switch_token != snapshot.switch_token
    {
        return Err(AppError::StalePlacementGeneration(
            "激活前 placement 或迁移围栏身份已变化".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../tests/tenant_data_migration/activation.rs"]
mod tests;
