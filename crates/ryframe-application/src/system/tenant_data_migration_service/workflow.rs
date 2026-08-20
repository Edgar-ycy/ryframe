use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use ryframe_db::{
    TenantRepository, background_job, tenant_data_migration, tenant_data_placement,
    tenant_operation_lease,
};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{DatabaseTransaction, TransactionTrait};
use serde_json::Value as JsonValue;

use crate::{JobHandler, TenantDataFence};

use super::recovery::RecoveryIntent;
use super::{
    OPERATION_LEASE_HOURS, TenantDataMigrationService, checked_generation, database_error,
};

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

    async fn handle(&self, job: &background_job::Model) -> AppResult<()> {
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
                let snapshot = self
                    .service
                    .repository
                    .migration(self.service.database.write(), migration_id)
                    .await?;
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
                        tenant_data_migration::Model::STATE_CUTTING_OVER
                            | tenant_data_migration::Model::STATE_ACTIVATING
                            | tenant_data_migration::Model::STATE_SUCCEEDED
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
            .repository
            .migration(self.database.write(), migration_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户数据迁移不存在".into()))?;
        self.ensure_migration_contract(&migration)?;
        let needs_execution_lease = !matches!(
            migration.state.as_str(),
            tenant_data_migration::Model::STATE_FINALIZED
                | tenant_data_migration::Model::STATE_FAILED
                | tenant_data_migration::Model::STATE_CANCELLED
        ) && (migration.state
            != tenant_data_migration::Model::STATE_RETENTION_PENDING
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
            if migration.state != tenant_data_migration::Model::STATE_CANCELLED {
                return Err(AppError::TenantOperationConflict(
                    "取消 intent 与迁移状态不一致".into(),
                ));
            }
        }
        if migration.finalize_idempotency_key_hash.is_some()
            || migration.finalize_requested_at.is_some()
        {
            if migration.state == tenant_data_migration::Model::STATE_RETENTION_PENDING {
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
            if migration.state != tenant_data_migration::Model::STATE_FINALIZED {
                return Err(AppError::TenantOperationConflict(
                    "finalize intent 与迁移状态不一致".into(),
                ));
            }
        }
        if matches!(
            migration.state.as_str(),
            tenant_data_migration::Model::STATE_FINALIZED
                | tenant_data_migration::Model::STATE_FAILED
                | tenant_data_migration::Model::STATE_CANCELLED
                | tenant_data_migration::Model::STATE_RETENTION_PENDING
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

        if migration.state == tenant_data_migration::Model::STATE_PRECHECKING {
            self.assert_worker_can_run(&migration, tenant_data_migration::Model::STATE_PRECHECKING)
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
                    tenant_data_migration::Model::STATE_QUEUED,
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
        if migration.state == tenant_data_migration::Model::STATE_QUEUED {
            migration = self.enter_maintenance(migration).await?;
        }
        if migration.state == tenant_data_migration::Model::STATE_QUIESCING {
            self.assert_worker_can_run(&migration, tenant_data_migration::Model::STATE_QUIESCING)
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
                    tenant_data_migration::Model::STATE_FROZEN,
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
        if migration.state == tenant_data_migration::Model::STATE_FROZEN {
            migration = self
                .set_state(
                    migration,
                    tenant_data_migration::Model::STATE_COPYING,
                    |model, now| model.copy_started_at = Some(now),
                )
                .await?;
        }
        if migration.state == tenant_data_migration::Model::STATE_COPYING {
            self.assert_worker_can_run(&migration, tenant_data_migration::Model::STATE_COPYING)
                .await?;
            self.copy_catalog(&migration).await?;
            migration = self
                .set_state(
                    migration,
                    tenant_data_migration::Model::STATE_VERIFYING,
                    |model, now| model.copy_completed_at = Some(now),
                )
                .await?;
        }
        if migration.state == tenant_data_migration::Model::STATE_VERIFYING {
            self.assert_worker_can_run(&migration, tenant_data_migration::Model::STATE_VERIFYING)
                .await?;
            self.verify_catalog(&migration).await?;
            migration = self
                .set_state(
                    migration,
                    tenant_data_migration::Model::STATE_CUTTING_OVER,
                    |model, now| model.verified_at = Some(now),
                )
                .await?;
        }
        if migration.state == tenant_data_migration::Model::STATE_CUTTING_OVER {
            migration = self.cut_over(migration).await?;
        }
        if migration.state == tenant_data_migration::Model::STATE_ACTIVATING {
            self.assert_worker_can_run(&migration, tenant_data_migration::Model::STATE_ACTIVATING)
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
        if migration.state == tenant_data_migration::Model::STATE_SUCCEEDED {
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
        snapshot: tenant_data_migration::Model,
    ) -> AppResult<tenant_data_migration::Model> {
        let now = self
            .lease_repository
            .database_utc_now(self.database.write())
            .await?;
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        self.acquire_or_renew_operation_lease(&transaction, &snapshot, now)
            .await?;
        let tenant = self
            .lease_repository
            .lock_tenant_and_validate_in_txn(
                &transaction,
                &snapshot.tenant_id,
                Some(&snapshot.switch_token),
            )
            .await?;
        let mut placement = self
            .repository
            .lock_placement_in_txn(&transaction, &snapshot.tenant_id)
            .await?;
        let mut migration = self
            .repository
            .lock_migration_in_txn(&transaction, snapshot.id)
            .await?;
        ensure_not_cancel_requested(&migration)?;
        if migration.state == tenant_data_migration::Model::STATE_QUIESCING {
            transaction.commit().await.map_err(database_error)?;
            return Ok(migration);
        }
        if migration.state != tenant_data_migration::Model::STATE_QUEUED {
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
        let changed = placement.state == tenant_data_placement::Model::STATE_ACTIVE;
        if changed {
            placement.state = tenant_data_placement::Model::STATE_MAINTENANCE.into();
            placement.updated_at = now;
            self.repository
                .save_placement_in_txn(&transaction, placement)
                .await?;
            TenantRepository
                .increment_runtime_epoch_in_txn(&transaction, &migration.tenant_id)
                .await?;
        } else if placement.state != tenant_data_placement::Model::STATE_MAINTENANCE {
            return Err(AppError::TenantOperationConflict(
                "placement 状态不允许进入维护".into(),
            ));
        }
        migration.state = tenant_data_migration::Model::STATE_QUIESCING.into();
        migration.quiesced_at.get_or_insert(now);
        migration.updated_at = now;
        migration = self
            .repository
            .save_migration_in_txn(&transaction, migration)
            .await?;
        transaction.commit().await.map_err(database_error)?;
        if changed {
            self.authorization_cache
                .publish_tenant_context_changed(&migration.tenant_id, tenant.authorization_epoch)
                .await;
        }
        Ok(migration)
    }

    async fn cut_over(
        &self,
        snapshot: tenant_data_migration::Model,
    ) -> AppResult<tenant_data_migration::Model> {
        // CUTTING_OVER is durable and may be resumed after a crash. Re-establish the
        // complete cross-database safety proof immediately before changing the control
        // pointer; no control transaction is held across these target-database awaits.
        self.targets.verify_now(&snapshot.source_target_key).await?;
        self.targets.verify_now(&snapshot.target_key).await?;
        self.assert_migration_frozen_fence(&snapshot, &snapshot.source_target_key)
            .await?;
        self.assert_migration_frozen_fence(&snapshot, &snapshot.target_key)
            .await?;
        let now = self
            .lease_repository
            .database_utc_now(self.database.write())
            .await?;
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        self.renew_operation_lease(&transaction, &snapshot, now)
            .await?;
        let tenant = self
            .lease_repository
            .lock_tenant_and_validate_in_txn(
                &transaction,
                &snapshot.tenant_id,
                Some(&snapshot.switch_token),
            )
            .await?;
        let mut placement = self
            .repository
            .lock_placement_in_txn(&transaction, &snapshot.tenant_id)
            .await?;
        let mut migration = self
            .repository
            .lock_migration_in_txn(&transaction, snapshot.id)
            .await?;
        ensure_not_cancel_requested(&migration)?;
        if migration.state == tenant_data_migration::Model::STATE_ACTIVATING {
            transaction.commit().await.map_err(database_error)?;
            return Ok(migration);
        }
        if migration.state != tenant_data_migration::Model::STATE_CUTTING_OVER {
            return Err(AppError::TenantOperationConflict(
                "迁移状态不允许执行 cutover".into(),
            ));
        }
        if placement.state != tenant_data_placement::Model::STATE_MAINTENANCE
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
        self.repository
            .save_placement_in_txn(&transaction, placement)
            .await?;
        TenantRepository
            .increment_runtime_epoch_in_txn(&transaction, &migration.tenant_id)
            .await?;
        migration.state = tenant_data_migration::Model::STATE_ACTIVATING.into();
        migration.cut_over_at.get_or_insert(now);
        migration.updated_at = now;
        migration = self
            .repository
            .save_migration_in_txn(&transaction, migration)
            .await?;
        transaction.commit().await.map_err(database_error)?;
        self.authorization_cache
            .publish_tenant_context_changed(&migration.tenant_id, tenant.authorization_epoch)
            .await;
        Ok(migration)
    }

    async fn activate_control(
        &self,
        snapshot: tenant_data_migration::Model,
    ) -> AppResult<tenant_data_migration::Model> {
        let now = self
            .lease_repository
            .database_utc_now(self.database.write())
            .await?;
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        self.renew_operation_lease(&transaction, &snapshot, now)
            .await?;
        let tenant = self
            .lease_repository
            .lock_tenant_and_validate_in_txn(
                &transaction,
                &snapshot.tenant_id,
                Some(&snapshot.switch_token),
            )
            .await?;
        let mut placement = self
            .repository
            .lock_placement_in_txn(&transaction, &snapshot.tenant_id)
            .await?;
        let mut migration = self
            .repository
            .lock_migration_in_txn(&transaction, snapshot.id)
            .await?;
        ensure_not_cancel_requested(&migration)?;
        if migration.state == tenant_data_migration::Model::STATE_SUCCEEDED {
            transaction.commit().await.map_err(database_error)?;
            return Ok(migration);
        }
        if migration.state != tenant_data_migration::Model::STATE_ACTIVATING {
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
        let changed = placement.state != tenant_data_placement::Model::STATE_ACTIVE;
        placement.state = tenant_data_placement::Model::STATE_ACTIVE.into();
        placement.updated_at = now;
        self.repository
            .save_placement_in_txn(&transaction, placement)
            .await?;
        if changed {
            TenantRepository
                .increment_runtime_epoch_in_txn(&transaction, &migration.tenant_id)
                .await?;
        }
        migration.state = tenant_data_migration::Model::STATE_SUCCEEDED.into();
        migration.activated_at.get_or_insert(now);
        migration.succeeded_at.get_or_insert(now);
        migration.updated_at = now;
        migration = self
            .repository
            .save_migration_in_txn(&transaction, migration)
            .await?;
        transaction.commit().await.map_err(database_error)?;
        if changed {
            self.authorization_cache
                .publish_tenant_context_changed(&migration.tenant_id, tenant.authorization_epoch)
                .await;
        }
        Ok(migration)
    }

    async fn enter_retention(
        &self,
        snapshot: tenant_data_migration::Model,
    ) -> AppResult<tenant_data_migration::Model> {
        let now = self
            .lease_repository
            .database_utc_now(self.database.write())
            .await?;
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        self.renew_operation_lease(&transaction, &snapshot, now)
            .await?;
        let mut migration = self
            .repository
            .lock_migration_in_txn(&transaction, snapshot.id)
            .await?;
        if migration.state == tenant_data_migration::Model::STATE_RETENTION_PENDING {
            transaction.commit().await.map_err(database_error)?;
            return Ok(migration);
        }
        if migration.state != tenant_data_migration::Model::STATE_SUCCEEDED {
            return Err(AppError::TenantOperationConflict(
                "迁移状态不允许进入 retention_pending".into(),
            ));
        }
        migration.state = tenant_data_migration::Model::STATE_RETENTION_PENDING.into();
        migration.retention_until = Some(
            migration.succeeded_at.unwrap_or(now)
                + Duration::hours(i64::from(migration.retention_hours)),
        );
        migration.updated_at = now;
        migration = self
            .repository
            .save_migration_in_txn(&transaction, migration)
            .await?;
        self.lease_repository
            .release_in_txn(&transaction, &migration.tenant_id, &migration.switch_token)
            .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(migration)
    }

    async fn set_state<F>(
        &self,
        snapshot: tenant_data_migration::Model,
        state: &str,
        update: F,
    ) -> AppResult<tenant_data_migration::Model>
    where
        F: FnOnce(&mut tenant_data_migration::Model, chrono::DateTime<Utc>),
    {
        let now = self
            .lease_repository
            .database_utc_now(self.database.write())
            .await?;
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        self.renew_operation_lease(&transaction, &snapshot, now)
            .await?;
        let mut migration = self
            .repository
            .lock_migration_in_txn(&transaction, snapshot.id)
            .await?;
        ensure_not_cancel_requested(&migration)?;
        if migration.state == state {
            transaction.commit().await.map_err(database_error)?;
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
        migration = self
            .repository
            .save_migration_in_txn(&transaction, migration)
            .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(migration)
    }

    pub(super) async fn acquire_or_renew_operation_lease(
        &self,
        transaction: &DatabaseTransaction,
        migration: &tenant_data_migration::Model,
        now: chrono::DateTime<Utc>,
    ) -> AppResult<()> {
        self.lease_repository
            .acquire_in_txn(
                transaction,
                tenant_operation_lease::Model {
                    tenant_id: migration.tenant_id.clone(),
                    owner_token: migration.switch_token.clone(),
                    operation: "tenant_data.migration".into(),
                    resource_type: "tenant_data_migration".into(),
                    resource_id: migration.id.to_string(),
                    expires_at: now + Duration::hours(OPERATION_LEASE_HOURS),
                    created_at: now,
                    updated_at: now,
                },
            )
            .await?;
        Ok(())
    }

    fn ensure_migration_contract(&self, migration: &tenant_data_migration::Model) -> AppResult<()> {
        let fingerprint = self.catalog.schema_fingerprint();
        if migration.source_schema_fingerprint != fingerprint
            || migration.target_schema_fingerprint != fingerprint
        {
            return Err(AppError::TenantOperationConflict(
                "迁移编译期 catalog/schema 指纹与持久化计划不一致".into(),
            ));
        }
        let source_mode = self
            .router
            .targets()
            .target_mode_code(&migration.source_target_key)
            .ok_or_else(|| AppError::TenantDataTargetUnavailable("源目标未注册".into(), 5))?;
        let source_kind = self
            .router
            .targets()
            .target_kind_code(&migration.source_target_key)
            .ok_or_else(|| AppError::TenantDataTargetUnavailable("源目标未注册".into(), 5))?;
        let target_mode = self
            .router
            .targets()
            .target_mode_code(&migration.target_key)
            .ok_or_else(|| AppError::TenantDataTargetUnavailable("目标未注册".into(), 5))?;
        let target_kind = self
            .router
            .targets()
            .target_kind_code(&migration.target_key)
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
        migration: &tenant_data_migration::Model,
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
        snapshot: &tenant_data_migration::Model,
    ) -> AppResult<tenant_data_migration::Model> {
        let now = self
            .lease_repository
            .database_utc_now(self.database.write())
            .await?;
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        self.acquire_or_renew_operation_lease(&transaction, snapshot, now)
            .await?;
        let current = self
            .repository
            .lock_migration_in_txn(&transaction, snapshot.id)
            .await?;
        self.ensure_migration_contract(&current)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(current)
    }

    pub(super) async fn assert_worker_can_run(
        &self,
        snapshot: &tenant_data_migration::Model,
        expected_state: &str,
    ) -> AppResult<()> {
        let now = self
            .lease_repository
            .database_utc_now(self.database.write())
            .await?;
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        self.acquire_or_renew_operation_lease(&transaction, snapshot, now)
            .await?;
        let current = self
            .repository
            .lock_migration_in_txn(&transaction, snapshot.id)
            .await?;
        ensure_not_cancel_requested(&current)?;
        if current.state != expected_state {
            return Err(AppError::TenantOperationConflict(format!(
                "迁移状态已变化: expected={expected_state}, actual={}",
                current.state
            )));
        }
        transaction.commit().await.map_err(database_error)
    }

    pub(super) async fn renew_operation_lease(
        &self,
        transaction: &DatabaseTransaction,
        migration: &tenant_data_migration::Model,
        now: chrono::DateTime<Utc>,
    ) -> AppResult<()> {
        if self
            .lease_repository
            .renew_in_txn(
                transaction,
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

pub(super) fn ensure_not_cancel_requested(
    migration: &tenant_data_migration::Model,
) -> AppResult<()> {
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
    migration: &tenant_data_migration::Model,
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
            migration.state == tenant_data_migration::Model::STATE_RETENTION_PENDING
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

pub(super) fn migration_requires_worker(migration: &tenant_data_migration::Model) -> bool {
    !matches!(
        migration.state.as_str(),
        tenant_data_migration::Model::STATE_FINALIZED
            | tenant_data_migration::Model::STATE_FAILED
            | tenant_data_migration::Model::STATE_CANCELLED
            | tenant_data_migration::Model::STATE_RETENTION_PENDING
    ) || (migration.state == tenant_data_migration::Model::STATE_RETENTION_PENDING
        && migration.finalize_requested_at.is_some())
}
