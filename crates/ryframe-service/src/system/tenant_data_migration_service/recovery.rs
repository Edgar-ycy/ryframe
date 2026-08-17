use ryframe_db::{TenantRepository, tenant_data_migration_item, tenant_data_placement};
use ryframe_tenant_db::TenantDataCleanupOwnership;
use sea_orm::TransactionTrait;

use super::workflow::{migration_requires_worker, validate_recovery_intent};
use super::*;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum RecoveryIntent {
    Cancel,
    Failure,
    Finalize,
}

impl TenantDataMigrationService {
    pub(super) async fn reconcile_authoritative_jobs(&self) -> AppResult<()> {
        let mut after_id = None;
        let mut changed = false;
        loop {
            let migrations = self
                .repository
                .recoverable_migrations(self.database.write(), after_id, 100)
                .await?;
            if migrations.is_empty() {
                break;
            }
            for migration in &migrations {
                match self.repair_authoritative_job(migration).await {
                    Ok(repaired) => changed |= repaired,
                    Err(error) => tracing::warn!(
                        migration_id = migration.id,
                        error_code = %error.error_code(),
                        "tenant-data migration watchdog 单项对账尚未完成"
                    ),
                }
            }
            after_id = migrations.last().map(|migration| migration.id);
            if migrations.len() < 100 {
                break;
            }
        }
        if changed {
            self.queue.notify_background_jobs().await;
        }
        Ok(())
    }

    async fn repair_authoritative_job(
        &self,
        snapshot: &tenant_data_migration::Model,
    ) -> AppResult<bool> {
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
        let mut migration = self
            .repository
            .lock_migration_in_txn(&transaction, snapshot.id)
            .await?;
        if !migration_requires_worker(&migration) {
            self.lease_repository
                .release_in_txn(&transaction, &migration.tenant_id, &migration.switch_token)
                .await?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(false);
        }
        if let Some(job_id) = migration.background_job_id
            && self
                .queue
                .reactivate_linked_in_transaction(
                    &transaction,
                    job_id,
                    super::TENANT_DATA_MIGRATION_JOB_TYPE,
                    "migration_id",
                    migration.id,
                    now,
                )
                .await?
        {
            transaction.commit().await.map_err(database_error)?;
            return Ok(false);
        }
        let recovery_generation = migration
            .background_job_id
            .map_or_else(|| "missing".into(), |job_id| job_id.to_string());
        let queued = self
            .queue
            .enqueue_in_transaction(
                &transaction,
                EnqueueBackgroundJob {
                    tenant_id: Some(migration.tenant_id.clone()),
                    schedule_id: None,
                    scheduled_for: None,
                    max_runtime_seconds: Some(86_400),
                    job_type: super::TENANT_DATA_MIGRATION_JOB_TYPE.into(),
                    payload: json!({ "migration_id": migration.id.to_string() }),
                    priority: 20,
                    available_at: now,
                    max_attempts: 8,
                    dedupe_key: Some(format!(
                        "migration-recovery:{}:{recovery_generation}:{}",
                        migration.id,
                        migration.updated_at.timestamp_micros()
                    )),
                    traceparent: None,
                    tracestate: None,
                },
            )
            .await?;
        migration.background_job_id = Some(queued.job.id);
        migration.updated_at = now;
        self.repository
            .save_migration_in_txn(&transaction, migration)
            .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(true)
    }
}

impl TenantDataMigrationService {
    pub(super) async fn reconcile_cancel_intent(
        &self,
        snapshot: tenant_data_migration::Model,
    ) -> AppResult<tenant_data_migration::Model> {
        validate_recovery_intent(&snapshot, RecoveryIntent::Cancel)?;
        // Worker 是唯一跨库副作用执行者。先恢复源 fence 和 control
        // placement 可用性，再按反向 FK 顺序分批清理目标，完成后才写 CANCELLED。
        self.router
            .activate_fence_for_catalog(
                &snapshot.tenant_id,
                &snapshot.source_target_key,
                checked_generation(snapshot.source_generation, "源")?,
                &snapshot.source_switch_token,
                &self.catalog,
            )
            .await
            .map_err(crate::map_tenant_data_error)?;
        let snapshot = self
            .restore_source_placement(&snapshot, RecoveryIntent::Cancel)
            .await?;
        self.cleanup_catalog_rows(
            &snapshot,
            RecoveryIntent::Cancel,
            &snapshot.target_key,
            snapshot.target_generation,
            &snapshot.switch_token,
        )
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
        self.acquire_or_renew_operation_lease(&transaction, &snapshot, now)
            .await?;
        let placement = self
            .repository
            .lock_placement_in_txn(&transaction, &snapshot.tenant_id)
            .await?;
        let mut migration = self
            .repository
            .lock_migration_in_txn(&transaction, snapshot.id)
            .await?;
        validate_recovery_intent(&migration, RecoveryIntent::Cancel)?;
        if placement.current_target_key != migration.source_target_key
            || placement.placement_generation != migration.source_generation
            || placement.switch_token != migration.source_switch_token
            || placement.state != tenant_data_placement::Model::STATE_ACTIVE
        {
            return Err(AppError::StalePlacementGeneration(
                "取消收口时源 placement 已变化".into(),
            ));
        }
        migration.state = tenant_data_migration::Model::STATE_CANCELLED.into();
        migration.cancelled_at = Some(now);
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

    pub(super) async fn reconcile_finalize_intent(
        &self,
        snapshot: tenant_data_migration::Model,
    ) -> AppResult<tenant_data_migration::Model> {
        validate_recovery_intent(&snapshot, RecoveryIntent::Finalize)?;
        let now = self
            .lease_repository
            .database_utc_now(self.database.write())
            .await?;
        if snapshot.retention_until.is_none_or(|until| until > now) {
            return Err(AppError::TenantOperationConflict(
                "源数据保留期尚未结束".into(),
            ));
        }
        let not_before = snapshot
            .activated_at
            .or(snapshot.succeeded_at)
            .ok_or_else(|| AppError::Conflict("迁移缺少激活时间".into()))?;
        if self
            .repository
            .validated_backup_for_destination(self.database.write(), &snapshot, not_before, now)
            .await?
            .is_none()
        {
            return Err(AppError::TenantOperationConflict(
                "validated backup 不再满足 finalize 条件".into(),
            ));
        }
        self.cleanup_catalog_rows(
            &snapshot,
            RecoveryIntent::Finalize,
            &snapshot.source_target_key,
            snapshot.source_generation,
            &snapshot.source_switch_token,
        )
        .await?;

        // 外部分批清理完成后重新读取 DB 时钟并重建/续租，不复用旧 now。
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
        let placement = self
            .repository
            .lock_placement_in_txn(&transaction, &snapshot.tenant_id)
            .await?;
        let mut migration = self
            .repository
            .lock_migration_in_txn(&transaction, snapshot.id)
            .await?;
        validate_recovery_intent(&migration, RecoveryIntent::Finalize)?;
        if placement.current_target_key != migration.target_key
            || placement.placement_generation != migration.target_generation
            || placement.switch_token != migration.switch_token
            || placement.state != tenant_data_placement::Model::STATE_ACTIVE
        {
            return Err(AppError::StalePlacementGeneration(
                "finalize 收口时当前 placement 已变化".into(),
            ));
        }
        migration.state = tenant_data_migration::Model::STATE_FINALIZED.into();
        migration.finalized_at = Some(now);
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
}

impl TenantDataMigrationService {
    async fn assert_recovery_can_run(
        &self,
        snapshot: &tenant_data_migration::Model,
        intent: RecoveryIntent,
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
        validate_recovery_intent(&current, intent)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(current)
    }

    async fn restore_source_placement(
        &self,
        snapshot: &tenant_data_migration::Model,
        intent: RecoveryIntent,
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
        let migration = self
            .repository
            .lock_migration_in_txn(&transaction, snapshot.id)
            .await?;
        validate_recovery_intent(&migration, intent)?;
        if placement.current_target_key != migration.source_target_key
            || placement.placement_generation != migration.source_generation
            || placement.switch_token != migration.source_switch_token
            || !matches!(
                placement.state.as_str(),
                tenant_data_placement::Model::STATE_ACTIVE
                    | tenant_data_placement::Model::STATE_MAINTENANCE
            )
        {
            return Err(AppError::StalePlacementGeneration(
                "恢复源数据时 placement 已变化".into(),
            ));
        }
        let changed = placement.state != tenant_data_placement::Model::STATE_ACTIVE;
        if changed {
            placement.state = tenant_data_placement::Model::STATE_ACTIVE.into();
            placement.updated_at = now;
            self.repository
                .save_placement_in_txn(&transaction, placement)
                .await?;
            TenantRepository
                .increment_runtime_epoch_in_txn(&transaction, &migration.tenant_id)
                .await?;
        }
        transaction.commit().await.map_err(database_error)?;
        if changed {
            self.authorization_cache
                .publish_tenant_context_changed(&migration.tenant_id, tenant.authorization_epoch)
                .await;
        }
        Ok(migration)
    }

    async fn cleanup_catalog_rows(
        &self,
        snapshot: &tenant_data_migration::Model,
        intent: RecoveryIntent,
        target_key: &str,
        generation: i64,
        switch_token: &str,
    ) -> AppResult<()> {
        let generation = checked_generation(generation, "清理")?;
        match self
            .router
            .cleanup_ownership_for_catalog(
                &snapshot.tenant_id,
                target_key,
                generation,
                switch_token,
                &self.catalog,
            )
            .await
            .map_err(crate::map_tenant_data_error)?
        {
            TenantDataCleanupOwnership::OwnedFrozen => {}
            TenantDataCleanupOwnership::AlreadyClean => return Ok(()),
            TenantDataCleanupOwnership::NotOwned
                if snapshot.state == tenant_data_migration::Model::STATE_PRECHECKING
                    && matches!(intent, RecoveryIntent::Cancel | RecoveryIntent::Failure) =>
            {
                // precheck/prepare 前的取消或失败对目标没有任何所有权。
                // 保留陌生 fence/数据并收口本迁移，绝不将其视为可清理副作用。
                tracing::warn!(
                    migration_id = snapshot.id,
                    target = target_key,
                    "precheck 迁移未拥有目标 cleanup fence，已安全跳过陌生数据"
                );
                return Ok(());
            }
            TenantDataCleanupOwnership::NotOwned
                if self
                    .repository
                    .migration(self.database.write(), snapshot.id)
                    .await?
                    .is_some_and(|migration| migration.cleanup_ready_at.is_some()) =>
            {
                // 所有 catalog 批次清空后会先持久化 cleanup_ready_at，再移除目标
                // fence/slot。若进程恰在两者之间崩溃且 dedicated 槽已被新租户占用，
                // 此检查点只授权控制面收口，绝不再次触碰新租户目标数据。
                return Ok(());
            }
            TenantDataCleanupOwnership::NotOwned => {
                return Err(AppError::StalePlacementGeneration(
                    "cleanup fence/slot 不属于当前 migration".into(),
                ));
            }
        }
        for descriptor in self.catalog.tables().iter().rev() {
            let existing_item = self
                .repository
                .items(self.database.write(), snapshot.id)
                .await?
                .into_iter()
                .find(|item| item.table_name == descriptor.table);
            let mut item = if let Some(item) = existing_item {
                item
            } else {
                let now = self
                    .lease_repository
                    .database_utc_now(self.database.write())
                    .await?;
                self.repository
                    .insert_item(
                        self.database.write(),
                        tenant_data_migration_item::Model {
                            id: ryframe_utils::snowflake::try_next_snowflake_id()?,
                            migration_id: snapshot.id,
                            table_name: descriptor.table.into(),
                            copy_order: i32::try_from(descriptor.copy_order).map_err(|_| {
                                AppError::Validation("catalog copy_order 超出范围".into())
                            })?,
                            state: tenant_data_migration_item::Model::STATE_PENDING.into(),
                            cursor_json: None,
                            source_row_count: Some(0),
                            target_row_count: Some(0),
                            source_digest: None,
                            target_digest: None,
                            error_code: None,
                            error_detail: None,
                            copy_started_at: None,
                            copied_at: None,
                            verified_at: None,
                            cleanup_state: tenant_data_migration_item::Model::CLEANUP_PENDING
                                .into(),
                            cleanup_row_count: 0,
                            created_at: now,
                            updated_at: now,
                        },
                    )
                    .await?
            };
            if item.cleanup_state == tenant_data_migration_item::Model::CLEANUP_CLEANED {
                continue;
            }
            loop {
                self.assert_recovery_can_run(snapshot, intent).await?;
                let deleted = self
                    .router
                    .delete_tenant_rows_batch_for_catalog(
                        ryframe_tenant_db::TenantDataCleanupBatch {
                            tenant_id: &snapshot.tenant_id,
                            target_key,
                            placement_generation: generation,
                            switch_token,
                            descriptor,
                            batch_size: 500,
                        },
                        &self.catalog,
                    )
                    .await
                    .map_err(crate::map_tenant_data_error)?;
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
                validate_recovery_intent(&current, intent)?;
                item = self
                    .repository
                    .lock_item_in_txn(&transaction, item.id)
                    .await?;
                item.cleanup_state = if deleted < 500 {
                    tenant_data_migration_item::Model::CLEANUP_CLEANED.into()
                } else {
                    tenant_data_migration_item::Model::CLEANUP_CLEANING.into()
                };
                item.cleanup_row_count = item
                    .cleanup_row_count
                    .checked_add(
                        i64::try_from(deleted)
                            .map_err(|_| AppError::Internal("cleanup row count overflow".into()))?,
                    )
                    .ok_or_else(|| AppError::Internal("cleanup row count overflow".into()))?;
                item.updated_at = now;
                item = self.repository.save_item(&transaction, item).await?;
                transaction.commit().await.map_err(database_error)?;
                if deleted < 500 {
                    break;
                }
            }
        }
        self.assert_recovery_can_run(snapshot, intent).await?;
        let checkpoint_now = self
            .lease_repository
            .database_utc_now(self.database.write())
            .await?;
        let checkpoint_transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        self.acquire_or_renew_operation_lease(&checkpoint_transaction, snapshot, checkpoint_now)
            .await?;
        let mut checkpoint = self
            .repository
            .lock_migration_in_txn(&checkpoint_transaction, snapshot.id)
            .await?;
        validate_recovery_intent(&checkpoint, intent)?;
        if checkpoint.cleanup_ready_at.is_none() {
            checkpoint.cleanup_ready_at = Some(checkpoint_now);
            checkpoint.updated_at = checkpoint_now;
            self.repository
                .save_migration_in_txn(&checkpoint_transaction, checkpoint)
                .await?;
        }
        checkpoint_transaction
            .commit()
            .await
            .map_err(database_error)?;
        self.router
            .finish_tenant_cleanup_for_catalog(
                &snapshot.tenant_id,
                target_key,
                generation,
                switch_token,
                &self.catalog,
            )
            .await
            .map_err(crate::map_tenant_data_error)
    }

    pub(super) async fn compensate_before_cutover(
        &self,
        migration_id: i64,
        error_code: &str,
    ) -> AppResult<()> {
        let Some(mut snapshot) = self
            .repository
            .migration(self.database.write(), migration_id)
            .await?
        else {
            return Ok(());
        };
        if !snapshot.can_cancel() {
            return Ok(());
        }
        let now = self
            .lease_repository
            .database_utc_now(self.database.write())
            .await?;
        // 先持久化补偿意图，阻止任何并发 Worker 继续跃迁；目标不可用时后续任务可
        // 根据 error_code 恢复补偿，而不是把租户永久遗留在 maintenance/frozen。
        let intent_transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        self.acquire_or_renew_operation_lease(&intent_transaction, &snapshot, now)
            .await?;
        let mut intent = self
            .repository
            .lock_migration_in_txn(&intent_transaction, migration_id)
            .await?;
        if intent.cancel_idempotency_key_hash.is_some() {
            return Err(AppError::TenantOperationConflict(
                "用户取消已经取得迁移仲裁权".into(),
            ));
        }
        if !intent.can_cancel() {
            return Ok(());
        }
        if intent.error_code.is_none() {
            intent.error_code = Some(error_code.chars().take(64).collect());
            intent.error_detail = None;
            intent.updated_at = now;
            intent = self
                .repository
                .save_migration_in_txn(&intent_transaction, intent)
                .await?;
        }
        intent_transaction.commit().await.map_err(database_error)?;
        snapshot = intent;

        // 补偿由 Worker 串行：先恢复源 fence/placement，再按表分批清理
        // 目标；每批持久化进度并续租，全部收口后才写 FAILED。
        self.router
            .activate_fence_for_catalog(
                &snapshot.tenant_id,
                &snapshot.source_target_key,
                checked_generation(snapshot.source_generation, "源")?,
                &snapshot.source_switch_token,
                &self.catalog,
            )
            .await
            .map_err(crate::map_tenant_data_error)?;
        snapshot = self
            .restore_source_placement(&snapshot, RecoveryIntent::Failure)
            .await?;
        self.cleanup_catalog_rows(
            &snapshot,
            RecoveryIntent::Failure,
            &snapshot.target_key,
            snapshot.target_generation,
            &snapshot.switch_token,
        )
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
        self.acquire_or_renew_operation_lease(&transaction, &snapshot, now)
            .await?;
        let placement = self
            .repository
            .lock_placement_in_txn(&transaction, &snapshot.tenant_id)
            .await?;
        let mut migration = self
            .repository
            .lock_migration_in_txn(&transaction, migration_id)
            .await?;
        validate_recovery_intent(&migration, RecoveryIntent::Failure)?;
        if placement.current_target_key != migration.source_target_key
            || placement.placement_generation != migration.source_generation
            || placement.switch_token != migration.source_switch_token
            || placement.state != tenant_data_placement::Model::STATE_ACTIVE
        {
            return Err(AppError::StalePlacementGeneration(
                "失败补偿收口时源 placement 已变化".into(),
            ));
        }
        migration.state = tenant_data_migration::Model::STATE_FAILED.into();
        migration.failed_at = Some(now);
        migration.updated_at = now;
        migration = self
            .repository
            .save_migration_in_txn(&transaction, migration)
            .await?;
        self.lease_repository
            .release_in_txn(&transaction, &migration.tenant_id, &migration.switch_token)
            .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(())
    }
}
