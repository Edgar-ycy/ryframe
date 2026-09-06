use ryframe_kernel::{AppError, AppResult};

use crate::ports::tenant_data::{TenantDataFence, TenantDataMigrationRecord};

use super::{TenantDataMigrationService, checked_generation};

impl TenantDataMigrationService {
    pub(super) async fn resume_requested_recovery(
        &self,
        migration: TenantDataMigrationRecord,
    ) -> AppResult<Option<TenantDataMigrationRecord>> {
        let migration_id = migration.id;
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
                return Ok(None);
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
                return Ok(None);
            }
            if migration.state != TenantDataMigrationRecord::STATE_FINALIZED {
                return Err(AppError::TenantOperationConflict(
                    "finalize intent 与迁移状态不一致".into(),
                ));
            }
        }
        Ok(Some(migration))
    }

    pub(super) async fn prepare_migration_target(
        &self,
        mut migration: TenantDataMigrationRecord,
    ) -> AppResult<TenantDataMigrationRecord> {
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
        Ok(migration)
    }
}
