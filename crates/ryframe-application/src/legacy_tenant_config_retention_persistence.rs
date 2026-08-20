use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use ryframe_db::{
    ControlDatabaseCluster, DataRetentionRepository, FileRepository, TenantRepository,
    tenant_config_bundle, tenant_config_transfer,
};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::Set,
    ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait,
    sea_query::{Expr, LockType, SimpleExpr},
};

use crate::{
    PersistenceFuture, RetentionCleanupResult, TenantConfigArtifactCounts,
    TenantConfigRetentionPersistencePort,
};

const ACTIVE_TRANSFER_PREDICATE: &str = "NOT EXISTS (SELECT 1 FROM sys_tenant_config_transfer transfer WHERE transfer.tenant_id = sys_tenant_config_bundle.tenant_id AND transfer.bundle_id = sys_tenant_config_bundle.id AND transfer.status IN ('preview_pending', 'previewing', 'apply_pending', 'applying'))";
const INACTIVE_ROLLBACK_PREDICATE: &str =
    "sys_tenant_config_transfer.status NOT IN ('rollback_pending', 'rolling_back')";

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn TenantConfigRetentionPersistencePort> {
    Arc::new(LegacyTenantConfigRetentionPersistence {
        database,
        repository: DataRetentionRepository,
    })
}

struct LegacyTenantConfigRetentionPersistence {
    database: ControlDatabaseCluster,
    repository: DataRetentionRepository,
}

impl TenantConfigRetentionPersistencePort for LegacyTenantConfigRetentionPersistence {
    fn preview(&self, now: DateTime<Utc>) -> PersistenceFuture<'_, TenantConfigArtifactCounts> {
        Box::pin(async move { self.preview_counts(now).await })
    }

    fn cleanup_packages(
        &self,
        before: DateTime<Utc>,
        batch_size: usize,
        maximum: usize,
    ) -> PersistenceFuture<'_, RetentionCleanupResult> {
        Box::pin(async move {
            self.cleanup_expired_packages(before, batch_size, maximum)
                .await
        })
    }

    fn cleanup_snapshots(
        &self,
        before: DateTime<Utc>,
        batch_size: usize,
        maximum: usize,
    ) -> PersistenceFuture<'_, RetentionCleanupResult> {
        Box::pin(async move {
            self.cleanup_expired_snapshots(before, batch_size, maximum)
                .await
        })
    }
}

impl LegacyTenantConfigRetentionPersistence {
    async fn preview_counts(&self, now: DateTime<Utc>) -> AppResult<TenantConfigArtifactCounts> {
        let packages = tenant_config_bundle::Entity::find()
            .filter(tenant_config_bundle::Column::FileId.is_not_null())
            .filter(tenant_config_bundle::Column::ExpiresAt.lte(now))
            .filter(tenant_config_bundle::Column::Status.is_in([
                tenant_config_bundle::Model::STATUS_SUCCEEDED,
                tenant_config_bundle::Model::STATUS_FAILED,
                tenant_config_bundle::Model::STATUS_EXPIRED,
            ]))
            .filter(config_bundle_not_required_by_active_transfer())
            .count(self.database.write())
            .await
            .map_err(database_error)?;
        let snapshots = tenant_config_transfer::Entity::find()
            .filter(tenant_config_transfer::Column::SnapshotFileId.is_not_null())
            .filter(tenant_config_transfer::Column::RollbackExpiresAt.lte(now))
            .filter(config_snapshot_not_used_by_active_rollback())
            .count(self.database.write())
            .await
            .map_err(database_error)?;
        Ok(TenantConfigArtifactCounts {
            packages,
            snapshots,
        })
    }

    async fn cleanup_expired_packages(
        &self,
        before: DateTime<Utc>,
        batch_size: usize,
        maximum: usize,
    ) -> AppResult<RetentionCleanupResult> {
        let mut deleted = 0_u64;
        while usize::try_from(deleted).unwrap_or(usize::MAX) < maximum {
            let remaining_limit =
                maximum.saturating_sub(usize::try_from(deleted).unwrap_or(usize::MAX));
            let limit = batch_size.min(remaining_limit);
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
                .all(self.database.write())
                .await
                .map_err(database_error)?;
            if candidates.is_empty() {
                break;
            }
            let batch_len = candidates.len();
            for candidate in candidates {
                if self.detach_expired_package(candidate, before).await? {
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
            .count(self.database.write())
            .await
            .map_err(database_error)?;
        Ok(RetentionCleanupResult { deleted, remaining })
    }

    async fn detach_expired_package(
        &self,
        candidate: tenant_config_bundle::Model,
        before: DateTime<Utc>,
    ) -> AppResult<bool> {
        let Some(file_id) = candidate.file_id else {
            return Ok(false);
        };
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
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
        // 配置包与快照可能共享文件；仅最后一个引用消失的事务创建清理墓碑。
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

    async fn cleanup_expired_snapshots(
        &self,
        before: DateTime<Utc>,
        batch_size: usize,
        maximum: usize,
    ) -> AppResult<RetentionCleanupResult> {
        let mut deleted = 0_u64;
        while usize::try_from(deleted).unwrap_or(usize::MAX) < maximum {
            let remaining_limit =
                maximum.saturating_sub(usize::try_from(deleted).unwrap_or(usize::MAX));
            let limit = batch_size.min(remaining_limit);
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
                .all(self.database.write())
                .await
                .map_err(database_error)?;
            if candidates.is_empty() {
                break;
            }
            let batch_len = candidates.len();
            for candidate in candidates {
                if self.detach_expired_snapshot(candidate, before).await? {
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
            .count(self.database.write())
            .await
            .map_err(database_error)?;
        Ok(RetentionCleanupResult { deleted, remaining })
    }

    async fn detach_expired_snapshot(
        &self,
        candidate: tenant_config_transfer::Model,
        before: DateTime<Utc>,
    ) -> AppResult<bool> {
        let Some(file_id) = candidate.snapshot_file_id else {
            return Ok(false);
        };
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
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
        // 回滚快照沿用同一引用计数规则，避免误删仍被其他记录使用的文件。
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

fn config_bundle_not_required_by_active_transfer() -> SimpleExpr {
    Expr::cust(ACTIVE_TRANSFER_PREDICATE)
}

fn config_snapshot_not_used_by_active_rollback() -> SimpleExpr {
    Expr::cust(INACTIVE_ROLLBACK_PREDICATE)
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_transfer_predicate_fails_closed_for_every_running_state() {
        for status in [
            tenant_config_transfer::Model::STATUS_PREVIEW_PENDING,
            tenant_config_transfer::Model::STATUS_PREVIEWING,
            tenant_config_transfer::Model::STATUS_APPLY_PENDING,
            tenant_config_transfer::Model::STATUS_APPLYING,
        ] {
            assert!(ACTIVE_TRANSFER_PREDICATE.contains(status));
        }
    }

    #[test]
    fn active_rollback_predicate_protects_pending_and_running_snapshots() {
        assert!(
            INACTIVE_ROLLBACK_PREDICATE
                .contains(tenant_config_transfer::Model::STATUS_ROLLBACK_PENDING)
        );
        assert!(
            INACTIVE_ROLLBACK_PREDICATE
                .contains(tenant_config_transfer::Model::STATUS_ROLLING_BACK)
        );
    }
}
