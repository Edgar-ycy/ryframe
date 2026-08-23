use ryframe_kernel::{ActorContext, AppError, AppResult};

use crate::ports::tenant_data::TenantDataMigrationRecord;

use super::{
    MigrationView, TenantDataMigrationService, ensure_platform_actor, validate_migration_tenant,
};

impl TenantDataMigrationService {
    pub async fn migration(
        &self,
        actor: &ActorContext,
        migration_id: i64,
    ) -> AppResult<MigrationView> {
        ensure_platform_actor(actor)?;
        let migration = self
            .persistence
            .migration(migration_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户数据迁移不存在".into()))?;
        self.migration_view(migration).await
    }

    pub async fn migrations_for_tenant(
        &self,
        actor: &ActorContext,
        tenant_id: &str,
        limit: u64,
    ) -> AppResult<Vec<MigrationView>> {
        ensure_platform_actor(actor)?;
        validate_migration_tenant(tenant_id)?;
        let rows = self
            .persistence
            .migrations_for_tenant(tenant_id, limit.clamp(1, 100))
            .await?;
        let mut views = Vec::with_capacity(rows.len());
        for row in rows {
            views.push(self.migration_view(row).await?);
        }
        Ok(views)
    }

    pub(super) async fn migration_view(
        &self,
        migration: TenantDataMigrationRecord,
    ) -> AppResult<MigrationView> {
        let items = self.persistence.items(migration.id).await?;
        let now = self.persistence.database_now().await?;
        let mut can_finalize = false;
        let mut action_reasons = Vec::new();
        if migration.can_finalize() {
            if migration.retention_until.is_none_or(|until| until > now) {
                action_reasons.push("retention_period_not_elapsed".into());
            } else if let Some(not_before) = migration.activated_at.or(migration.succeeded_at) {
                if self
                    .persistence
                    .has_validated_backup(&migration, not_before, now)
                    .await?
                {
                    can_finalize = true;
                } else {
                    action_reasons.push("validated_backup_required".into());
                }
            } else {
                action_reasons.push("activation_timestamp_missing".into());
            }
        } else {
            action_reasons.push("migration_not_retention_pending".into());
        }
        let can_cancel = migration.can_cancel()
            && migration.cancel_requested_at.is_none()
            && migration.error_code.is_none();
        if !can_cancel {
            action_reasons.push(if migration.cancel_requested_at.is_some() {
                "cancel_requested".into()
            } else {
                "migration_past_cancel_boundary".into()
            });
        }
        if migration.finalize_requested_at.is_some()
            && migration.state == TenantDataMigrationRecord::STATE_RETENTION_PENDING
        {
            can_finalize = false;
            action_reasons.push("finalize_requested".into());
        }
        Ok(MigrationView::from_models(
            migration,
            items,
            can_cancel,
            can_finalize,
            action_reasons,
        ))
    }
}
