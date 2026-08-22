use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use ryframe_kernel::{AppError, AppResult};
use serde_json::Value as JsonValue;

use crate::{
    ClaimedBackgroundJob, JobHandler,
    ports::tenant_data::{
        TenantDataFence, TenantDataMigrationRecord, TenantDataMigrationTransaction,
        TenantDataPlacementRecord, TenantOperationLeaseRecord,
    },
};

use super::recovery::RecoveryIntent;
use super::{OPERATION_LEASE_HOURS, TenantDataMigrationService, checked_generation};

pub struct TenantDataMigrationJobHandler {
    service: Arc<TenantDataMigrationService>,
}

impl TenantDataMigrationJobHandler {
    pub fn new(service: Arc<TenantDataMigrationService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl JobHandler for TenantDataMigrationJobHandler {
    fn job_type(&self) -> &'static str {
        super::TENANT_DATA_MIGRATION_JOB_TYPE
    }

    async fn handle(&self, job: &ClaimedBackgroundJob) -> AppResult<()> {
        let migration_id = job
            .payload
            .get("migration_id")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| AppError::Validation("tenant_data_migration 任务载荷无效".into()))?
            .parse::<i64>()
            .map_err(|_| AppError::Validation("tenant_data_migration id 无效".into()))?;
        let result = self.service.execute_migration(migration_id).await;
        if let Err(error) = &result {
            let permanent = matches!(
                error,
                AppError::Validation(_)
                    | AppError::Conflict(_)
                    | AppError::NotFound(_)
                    | AppError::CapabilityUnavailable(_)
            );
            let terminal = permanent || job.attempts >= job.max_attempts;
            if terminal {
                let snapshot = self.service.persistence.migration(migration_id).await?;
                if let Some(snapshot) = snapshot {
                    if snapshot.can_cancel() {
                        if let Err(compensation_error) = self
                            .service
                            .compensate_before_cutover(migration_id, error.error_code().as_str())
                            .await
                        {
                            tracing::warn!(
                                migration_id,
                                error_code = %compensation_error.error_code(),
                                "迁移失败补偿尚未完成，将不消耗尝试预算继续恢复"
                            );
                            return Err(AppError::RetryableConflict(
                                "tenant-data compensation pending".into(),
                                5,
                            ));
                        }
                    } else if matches!(
                        snapshot.state.as_str(),
                        TenantDataMigrationRecord::STATE_CUTTING_OVER
                            | TenantDataMigrationRecord::STATE_ACTIVATING
                            | TenantDataMigrationRecord::STATE_SUCCEEDED
                    ) {
                        // cutover 一旦开始就不再回源；即使普通尝试预算用尽，
                        // 也依赖 MySQL 任务行无限延后继续 forward-recovery。
                        return Err(AppError::RetryableConflict(
                            "tenant-data forward recovery pending".into(),
                            5,
                        ));
                    }
                }
            }
        }
        result
    }

    fn should_dead_letter(&self, error: &AppError) -> bool {
        matches!(
            error,
            AppError::Validation(_)
                | AppError::Conflict(_)
                | AppError::NotFound(_)
                | AppError::CapabilityUnavailable(_)
        )
    }

    fn has_authoritative_reconciler(&self) -> bool {
        true
    }

    async fn reconcile_authoritative_jobs(&self) -> AppResult<()> {
        self.service.reconcile_authoritative_jobs().await
    }
}

impl TenantDataMigrationService {
    pub(super) async fn execute_migration(&self, migration_id: i64) -> AppResult<()> {
        let mut migration = self
            .persistence
            .migration(migration_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户数据迁移不存在".into()))?;
        self.ensure_migration_contract(&migration)?;
        let needs_execution_lease = !matches!(
            migration.state.as_str(),
            TenantDataMigrationRecord::STATE_FINALIZED
                | TenantDataMigrationRecord::STATE_FAILED
                | TenantDataMigrationRecord::STATE_CANCELLED
        ) && (migration.state
            != TenantDataMigrationRecord::STATE_RETENTION_PENDING
            || migration.finalize_requested_at.is_some());
        if needs_execution_lease {
            migration = self.reacquire_execution_lease(&migration).await?;
        }
        if migration.cancel_idempotency_key_hash.is_some()
            || migration.cancel_requested_at.is_some()
        {
            if migration.can_cancel() {
                self.reconcile_cancel_intent(migration)
                    .await
                    .map_err(|error| {
                        tracing::warn!(
                            migration_id,
                            error_code = %error.error_code(),
                            "迁移取消恢复尚未完成"
                        );
                        AppError::RetryableConflict("tenant-data cancel recovery pending".into(), 5)
                    })?;
                return Ok(());
            }
            if migration.state != TenantDataMigrationRecord::STATE_CANCELLED {
                return Err(AppError::TenantOperationConflict(
                    "取消 intent 与迁移状态不一致".into(),
                ));
            }
        }
        if migration.finalize_idempotency_key_hash.is_some()
            || migration.finalize_requested_at.is_some()
        {
            if migration.state == TenantDataMigrationRecord::STATE_RETENTION_PENDING {
                self.reconcile_finalize_intent(migration)
                    .await
                    .map_err(|error| {
                        tracing::warn!(
                            migration_id,
                            error_code = %error.error_code(),
                            "迁移 finalize 恢复尚未完成"
                        );
                        AppError::RetryableConflict(
                            "tenant-data finalize recovery pending".into(),
                            5,
                        )
                    })?;
                return Ok(());
            }
            if migration.state != TenantDataMigrationRecord::STATE_FINALIZED {
                return Err(AppError::TenantOperationConflict(
                    "finalize intent 与迁移状态不一致".into(),
                ));
            }
        }
        if matches!(
            migration.state.as_str(),
            TenantDataMigrationRecord::STATE_FINALIZED
                | TenantDataMigrationRecord::STATE_FAILED
                | TenantDataMigrationRecord::STATE_CANCELLED
                | TenantDataMigrationRecord::STATE_RETENTION_PENDING
        ) {
            return Ok(());
        }
        if let Some(error_code) = migration.error_code.clone()
            && migration.can_cancel()
        {
            self.compensate_before_cutover(migration_id, &error_code)
                .await
                .map_err(|error| {
                    tracing::warn!(
                        migration_id,
                        error_code = %error.error_code(),
                        "迁移失败补偿尚未完成"
                    );
                    AppError::RetryableConflict(
                        "tenant-data compensation recovery pending".into(),
                        5,
                    )
                })?;
            return Ok(());
        }

        if migration.state == TenantDataMigrationRecord::STATE_PRECHECKING {
            self.assert_worker_can_run(&migration, TenantDataMigrationRecord::STATE_PRECHECKING)
                .await?;
            self.targets.verify_now(&migration.target_key).await?;
            self.tenant_migration
                .prepare_target(TenantDataFence {
                    tenant_id: &migration.tenant_id,
                    target_key: &migration.target_key,
                    generation: checked_generation(migration.target_generation, "目标")?,
                    switch_token: &migration.switch_token,
                })
                .await?;
            let transition = self
                .set_state(
                    migration.clone(),
                    TenantDataMigrationRecord::STATE_QUEUED,
                    |model, now| model.queued_at = Some(now),
                )
                .await;
            migration = match transition {
                Ok(migration) => migration,
                Err(error) => {
                    let _ = self
                        .tenant_migration
                        .clear_prepared_target(TenantDataFence {
                            tenant_id: &migration.tenant_id,
                            target_key: &migration.target_key,
                            generation: checked_generation(migration.target_generation, "目标")?,
                            switch_token: &migration.switch_token,
                        })
                        .await;
                    return Err(error);
                }
            };
        }
        if migration.state == TenantDataMigrationRecord::STATE_QUEUED {
            migration = self.enter_maintenance(migration).await?;
        }
        if migration.state == TenantDataMigrationRecord::STATE_QUIESCING {
            self.assert_worker_can_run(&migration, TenantDataMigrationRecord::STATE_QUIESCING)
                .await?;
            self.tenant_migration
                .freeze_fence(TenantDataFence {
                    tenant_id: &migration.tenant_id,
                    target_key: &migration.source_target_key,
                    generation: checked_generation(migration.source_generation, "源")?,
                    switch_token: &migration.source_switch_token,
                })
                .await?;
            let transition = self
                .set_state(
                    migration.clone(),
                    TenantDataMigrationRecord::STATE_FROZEN,
                    |model, now| model.frozen_at = Some(now),
                )
                .await;
            migration = match transition {
                Ok(migration) => migration,
                Err(error) => {
                    let _ = self
                        .tenant_migration
                        .activate_fence(TenantDataFence {
                            tenant_id: &migration.tenant_id,
                            target_key: &migration.source_target_key,
                            generation: checked_generation(migration.source_generation, "源")?,
                            switch_token: &migration.source_switch_token,
                        })
                        .await;
                    return Err(error);
                }
            };
        }
        if migration.state == TenantDataMigrationRecord::STATE_FROZEN {
            migration = self
                .set_state(
                    migration,
                    TenantDataMigrationRecord::STATE_COPYING,
                    |model, now| model.copy_started_at = Some(now),
                )
                .await?;
        }
        if migration.state == TenantDataMigrationRecord::STATE_COPYING {
            self.assert_worker_can_run(&migration, TenantDataMigrationRecord::STATE_COPYING)
                .await?;
            self.copy_catalog(&migration).await?;
            migration = self
                .set_state(
                    migration,
                    TenantDataMigrationRecord::STATE_VERIFYING,
                    |model, now| model.copy_completed_at = Some(now),
                )
                .await?;
        }
        if migration.state == TenantDataMigrationRecord::STATE_VERIFYING {
            self.assert_worker_can_run(&migration, TenantDataMigrationRecord::STATE_VERIFYING)
                .await?;
            self.verify_catalog(&migration).await?;
            migration = self
                .set_state(
                    migration,
                    TenantDataMigrationRecord::STATE_CUTTING_OVER,
                    |model, now| model.verified_at = Some(now),
                )
                .await?;
        }
        if migration.state == TenantDataMigrationRecord::STATE_CUTTING_OVER {
            migration = self.cut_over(migration).await?;
        }
        if migration.state == TenantDataMigrationRecord::STATE_ACTIVATING {
            self.assert_worker_can_run(&migration, TenantDataMigrationRecord::STATE_ACTIVATING)
                .await?;
            self.tenant_migration
                .activate_fence(TenantDataFence {
                    tenant_id: &migration.tenant_id,
                    target_key: &migration.target_key,
                    generation: checked_generation(migration.target_generation, "目标")?,
                    switch_token: &migration.switch_token,
                })
                .await?;
            migration = self.activate_control(migration).await?;
        }
        if migration.state == TenantDataMigrationRecord::STATE_SUCCEEDED {
            self.enter_retention(migration).await?;
            return Ok(());
        }
        Err(AppError::TenantOperationConflict(format!(
            "租户数据迁移状态未被 Worker 处理: {}",
            migration.state
        )))
    }

    async fn enter_maintenance(
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
            transaction.commit().await?;
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
        transaction.commit().await?;
        if changed {
            self.authorization_cache
                .publish_tenant_context_changed(&migration.tenant_id, tenant.authorization_epoch)
                .await;
        }
        Ok(migration)
    }

    async fn cut_over(
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
        self.renew_operation_lease(&*transaction, &snapshot, now)
            .await?;
        let tenant = transaction
            .lock_tenant(&snapshot.tenant_id, Some(&snapshot.switch_token))
            .await?;
        let mut placement = transaction.lock_placement(&snapshot.tenant_id).await?;
        let mut migration = transaction.lock_migration(snapshot.id).await?;
        ensure_not_cancel_requested(&migration)?;
        if migration.state == TenantDataMigrationRecord::STATE_ACTIVATING {
            transaction.commit().await?;
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
            .increment_runtime_epoch(&migration.tenant_id)
            .await?;
        migration.state = TenantDataMigrationRecord::STATE_ACTIVATING.into();
        migration.cut_over_at.get_or_insert(now);
        migration.updated_at = now;
        migration = transaction.save_migration(migration).await?;
        transaction.commit().await?;
        self.authorization_cache
            .publish_tenant_context_changed(&migration.tenant_id, tenant.authorization_epoch)
            .await;
        Ok(migration)
    }

    async fn activate_control(
        &self,
        snapshot: TenantDataMigrationRecord,
    ) -> AppResult<TenantDataMigrationRecord> {
        let now = self.persistence.database_now().await?;
        let transaction = self.persistence.begin().await?;
        self.renew_operation_lease(&*transaction, &snapshot, now)
            .await?;
        let tenant = transaction
            .lock_tenant(&snapshot.tenant_id, Some(&snapshot.switch_token))
            .await?;
        let mut placement = transaction.lock_placement(&snapshot.tenant_id).await?;
        let mut migration = transaction.lock_migration(snapshot.id).await?;
        ensure_not_cancel_requested(&migration)?;
        if migration.state == TenantDataMigrationRecord::STATE_SUCCEEDED {
            transaction.commit().await?;
            return Ok(migration);
        }
        if migration.state != TenantDataMigrationRecord::STATE_ACTIVATING {
            return Err(AppError::TenantOperationConflict(
                "迁移状态不允许激活 placement".into(),
            ));
        }
        if placement.current_target_key != migration.target_key
            || placement.placement_generation != migration.target_generation
            || placement.switch_token != migration.switch_token
        {
            return Err(AppError::StalePlacementGeneration(
                "激活前 placement 已变化".into(),
            ));
        }
        let changed = placement.state != TenantDataPlacementRecord::STATE_ACTIVE;
        placement.state = TenantDataPlacementRecord::STATE_ACTIVE.into();
        placement.updated_at = now;
        transaction.save_placement(placement).await?;
        if changed {
            transaction
                .increment_runtime_epoch(&migration.tenant_id)
                .await?;
        }
        migration.state = TenantDataMigrationRecord::STATE_SUCCEEDED.into();
        migration.activated_at.get_or_insert(now);
        migration.succeeded_at.get_or_insert(now);
        migration.updated_at = now;
        migration = transaction.save_migration(migration).await?;
        transaction.commit().await?;
        if changed {
            self.authorization_cache
                .publish_tenant_context_changed(&migration.tenant_id, tenant.authorization_epoch)
                .await;
        }
        Ok(migration)
    }

    async fn enter_retention(
        &self,
        snapshot: TenantDataMigrationRecord,
    ) -> AppResult<TenantDataMigrationRecord> {
        let now = self.persistence.database_now().await?;
        let transaction = self.persistence.begin().await?;
        self.renew_operation_lease(&*transaction, &snapshot, now)
            .await?;
        let mut migration = transaction.lock_migration(snapshot.id).await?;
        if migration.state == TenantDataMigrationRecord::STATE_RETENTION_PENDING {
            transaction.commit().await?;
            return Ok(migration);
        }
        if migration.state != TenantDataMigrationRecord::STATE_SUCCEEDED {
            return Err(AppError::TenantOperationConflict(
                "迁移状态不允许进入 retention_pending".into(),
            ));
        }
        migration.state = TenantDataMigrationRecord::STATE_RETENTION_PENDING.into();
        migration.retention_until = Some(
            migration.succeeded_at.unwrap_or(now)
                + Duration::hours(i64::from(migration.retention_hours)),
        );
        migration.updated_at = now;
        migration = transaction.save_migration(migration).await?;
        transaction
            .release_lease(&migration.tenant_id, &migration.switch_token)
            .await?;
        transaction.commit().await?;
        Ok(migration)
    }

    async fn set_state<F>(
        &self,
        snapshot: TenantDataMigrationRecord,
        state: &str,
        update: F,
    ) -> AppResult<TenantDataMigrationRecord>
    where
        F: FnOnce(&mut TenantDataMigrationRecord, chrono::DateTime<Utc>),
    {
        let now = self.persistence.database_now().await?;
        let transaction = self.persistence.begin().await?;
        self.renew_operation_lease(&*transaction, &snapshot, now)
            .await?;
        let mut migration = transaction.lock_migration(snapshot.id).await?;
        ensure_not_cancel_requested(&migration)?;
        if migration.state == state {
            transaction.commit().await?;
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
        transaction.commit().await?;
        Ok(migration)
    }

    pub(super) async fn acquire_or_renew_operation_lease(
        &self,
        transaction: &dyn TenantDataMigrationTransaction,
        migration: &TenantDataMigrationRecord,
        now: chrono::DateTime<Utc>,
    ) -> AppResult<()> {
        transaction
            .acquire_lease(TenantOperationLeaseRecord {
                tenant_id: migration.tenant_id.clone(),
                owner_token: migration.switch_token.clone(),
                operation: "tenant_data.migration".into(),
                resource_type: "tenant_data_migration".into(),
                resource_id: migration.id.to_string(),
                expires_at: now + Duration::hours(OPERATION_LEASE_HOURS),
                created_at: now,
                updated_at: now,
            })
            .await?;
        Ok(())
    }

    fn ensure_migration_contract(&self, migration: &TenantDataMigrationRecord) -> AppResult<()> {
        let fingerprint = self.targets.catalog_fingerprint();
        if migration.source_schema_fingerprint != fingerprint
            || migration.target_schema_fingerprint != fingerprint
        {
            return Err(AppError::TenantOperationConflict(
                "迁移编译期 catalog/schema 指纹与持久化计划不一致".into(),
            ));
        }
        let source_mode = self
            .targets
            .mode_code(&migration.source_target_key)
            .ok_or_else(|| AppError::TenantDataTargetUnavailable("源目标未注册".into(), 5))?;
        let source_kind = self
            .targets
            .kind_code(&migration.source_target_key)
            .ok_or_else(|| AppError::TenantDataTargetUnavailable("源目标未注册".into(), 5))?;
        let target_mode = self
            .targets
            .mode_code(&migration.target_key)
            .ok_or_else(|| AppError::TenantDataTargetUnavailable("目标未注册".into(), 5))?;
        let target_kind = self
            .targets
            .kind_code(&migration.target_key)
            .ok_or_else(|| AppError::TenantDataTargetUnavailable("目标未注册".into(), 5))?;
        if migration.source_target_mode != source_mode
            || migration.source_target_kind != source_kind
            || migration.target_target_mode != target_mode
            || migration.target_target_kind != target_kind
        {
            return Err(AppError::TenantOperationConflict(
                "迁移期间目标 mode/kind 配置发生漂移，已 fail-closed".into(),
            ));
        }
        Ok(())
    }

    pub(super) async fn assert_migration_frozen_fence(
        &self,
        migration: &TenantDataMigrationRecord,
        target_key: &str,
    ) -> AppResult<()> {
        let (generation, switch_token) = if target_key == migration.source_target_key {
            (
                migration.source_generation,
                migration.source_switch_token.as_str(),
            )
        } else if target_key == migration.target_key {
            (migration.target_generation, migration.switch_token.as_str())
        } else {
            return Err(AppError::TenantOperationConflict(
                "fence 断言目标不属于当前迁移".into(),
            ));
        };
        self.tenant_migration
            .assert_frozen_fence(TenantDataFence {
                tenant_id: &migration.tenant_id,
                target_key,
                generation: checked_generation(generation, "fence")?,
                switch_token,
            })
            .await
    }

    async fn reacquire_execution_lease(
        &self,
        snapshot: &TenantDataMigrationRecord,
    ) -> AppResult<TenantDataMigrationRecord> {
        let now = self.persistence.database_now().await?;
        let transaction = self.persistence.begin().await?;
        self.acquire_or_renew_operation_lease(&*transaction, snapshot, now)
            .await?;
        let current = transaction.lock_migration(snapshot.id).await?;
        self.ensure_migration_contract(&current)?;
        transaction.commit().await?;
        Ok(current)
    }

    pub(super) async fn assert_worker_can_run(
        &self,
        snapshot: &TenantDataMigrationRecord,
        expected_state: &str,
    ) -> AppResult<()> {
        let now = self.persistence.database_now().await?;
        let transaction = self.persistence.begin().await?;
        self.acquire_or_renew_operation_lease(&*transaction, snapshot, now)
            .await?;
        let current = transaction.lock_migration(snapshot.id).await?;
        ensure_not_cancel_requested(&current)?;
        if current.state != expected_state {
            return Err(AppError::TenantOperationConflict(format!(
                "迁移状态已变化: expected={expected_state}, actual={}",
                current.state
            )));
        }
        transaction.commit().await
    }

    pub(super) async fn renew_operation_lease(
        &self,
        transaction: &dyn TenantDataMigrationTransaction,
        migration: &TenantDataMigrationRecord,
        now: chrono::DateTime<Utc>,
    ) -> AppResult<()> {
        if transaction
            .renew_lease(
                &migration.tenant_id,
                &migration.switch_token,
                now + Duration::hours(OPERATION_LEASE_HOURS),
            )
            .await?
        {
            Ok(())
        } else {
            Err(AppError::TenantOperationConflict(
                "租户数据迁移租约已经丢失".into(),
            ))
        }
    }
}

pub(super) fn ensure_not_cancel_requested(migration: &TenantDataMigrationRecord) -> AppResult<()> {
    if migration.cancel_idempotency_key_hash.is_some() || migration.cancel_requested_at.is_some() {
        return Err(AppError::TenantOperationConflict(
            "租户数据迁移正在取消，Worker 不得继续状态跃迁".into(),
        ));
    }
    if migration.error_code.is_some() {
        return Err(AppError::TenantOperationConflict(
            "租户数据迁移正在执行失败补偿，Worker 不得继续状态跃迁".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_recovery_intent(
    migration: &TenantDataMigrationRecord,
    intent: RecoveryIntent,
) -> AppResult<()> {
    let valid = match intent {
        RecoveryIntent::Cancel => {
            migration.can_cancel()
                && migration.cancel_idempotency_key_hash.is_some()
                && migration.cancel_requested_at.is_some()
                && migration.error_code.is_none()
        }
        RecoveryIntent::Failure => {
            migration.can_cancel()
                && migration.error_code.is_some()
                && migration.cancel_idempotency_key_hash.is_none()
        }
        RecoveryIntent::Finalize => {
            migration.state == TenantDataMigrationRecord::STATE_RETENTION_PENDING
                && migration.finalize_idempotency_key_hash.is_some()
                && migration.finalize_requested_at.is_some()
        }
    };
    if valid {
        Ok(())
    } else {
        Err(AppError::TenantOperationConflict(
            "tenant-data recovery intent 与权威迁移状态不一致".into(),
        ))
    }
}

pub(super) fn migration_requires_worker(migration: &TenantDataMigrationRecord) -> bool {
    !matches!(
        migration.state.as_str(),
        TenantDataMigrationRecord::STATE_FINALIZED
            | TenantDataMigrationRecord::STATE_FAILED
            | TenantDataMigrationRecord::STATE_CANCELLED
            | TenantDataMigrationRecord::STATE_RETENTION_PENDING
    ) || (migration.state == TenantDataMigrationRecord::STATE_RETENTION_PENDING
        && migration.finalize_requested_at.is_some())
}
