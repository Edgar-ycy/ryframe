use super::*;
use ryframe_application::ports::tenant_data::TenantDataFence;
use ryframe_db::{
    entities::tenant::{data_migration, data_placement, operation_lease},
    repositories::database_utc_now_time,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect, sea_query::LockType};
use time::OffsetDateTime;

impl TenantDataScopeProjectionPublisher {
    pub(super) async fn publish_migration_projection(
        &self,
        fence: TenantDataFence<'_>,
    ) -> AppResult<i64> {
        let handle = self
            .router
            .open_target(fence.target_key)
            .await
            .map_err(map_error)?;
        let control = self
            .control
            .write()
            .begin_with_config(
                Some(IsolationLevel::RepeatableRead),
                Some(AccessMode::ReadOnly),
            )
            .await
            .db()?;
        let result = self
            .publish_locked_migration(&control, &handle, fence)
            .await;
        match result {
            Ok(version) => {
                control.commit().await.db()?;
                Ok(version)
            }
            Err(error) => {
                let _ = control.rollback().await;
                Err(error)
            }
        }
    }

    async fn publish_locked_migration(
        &self,
        control: &DatabaseTransaction,
        handle: &crate::TenantDataTargetHandle,
        fence: TenantDataFence<'_>,
    ) -> AppResult<i64> {
        let (version, expires_at) = lock_migration_context(control, fence).await?;
        let target = handle.connection().begin().await.db()?;
        let result = async {
            handle
                .lock_active_projection_fence(
                    &target,
                    fence.tenant_id,
                    fence.generation,
                    fence.switch_token,
                )
                .await
                .map_err(map_error)?;
            verification::publish_verified(control, &target, fence, version).await?;
            if expires_at <= database_utc_now_time(control).await? {
                return Err(AppError::TenantOperationConflict(
                    "迁移投影发布期间操作租约已过期".into(),
                ));
            }
            Ok(())
        }
        .await;
        match result {
            Ok(()) => {
                // 提交结果未知时保留控制面维护状态；下一次调用读取并完整重验目标。
                target.commit().await.db()?;
                Ok(version)
            }
            Err(error) => {
                let _ = target.rollback().await;
                Err(error)
            }
        }
    }
}

async fn lock_migration_context(
    control: &DatabaseTransaction,
    fence: TenantDataFence<'_>,
) -> AppResult<(i64, OffsetDateTime)> {
    // 与租户长操作保持相同锁序；共享租户锁固定版本及身份、组织数据快照。
    let tenant = TenantVersionRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT data_scope_version FROM sys_tenant WHERE tenant_id = ? LIMIT 1 FOR SHARE",
        [fence.tenant_id.into()],
    ))
    .one(control)
    .await
    .db()?
    .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
    let lease = operation_lease::Entity::find_by_id(fence.tenant_id.to_string())
        .lock(LockType::Share)
        .one(control)
        .await
        .db()?
        .ok_or_else(|| AppError::TenantOperationConflict("迁移投影缺少操作租约".into()))?;
    let placement = data_placement::Entity::find_by_id(fence.tenant_id.to_string())
        .lock(LockType::Share)
        .one(control)
        .await
        .db()?
        .ok_or_else(|| AppError::NotFound("迁移投影缺少 placement".into()))?;
    let migration_id = lease
        .resource_id
        .parse::<i64>()
        .map_err(|_| AppError::TenantOperationConflict("迁移租约资源标识无效".into()))?;
    let migration = data_migration::Entity::find_by_id(migration_id)
        .filter(data_migration::Column::TenantId.eq(fence.tenant_id))
        .lock(LockType::Share)
        .one(control)
        .await
        .db()?
        .ok_or_else(|| AppError::NotFound("迁移投影缺少权威迁移记录".into()))?;
    validate_migration_context(fence, &placement, &migration, &lease)?;
    if lease.expires_at <= database_utc_now_time(control).await? {
        return Err(AppError::TenantOperationConflict(
            "迁移投影操作租约已过期".into(),
        ));
    }
    Ok((tenant.data_scope_version, lease.expires_at))
}

fn validate_migration_context(
    fence: TenantDataFence<'_>,
    placement: &data_placement::Model,
    migration: &data_migration::Model,
    lease: &operation_lease::Model,
) -> AppResult<()> {
    if fence.generation <= 0
        || fence.switch_token.trim().is_empty()
        || placement.state != data_placement::Model::STATE_MAINTENANCE
        || placement.current_target_key != fence.target_key
        || placement.placement_generation != fence.generation
        || placement.switch_token != fence.switch_token
    {
        return Err(AppError::StalePlacementGeneration(
            "迁移投影要求精确匹配的 maintenance placement".into(),
        ));
    }
    if migration.state != data_migration::Model::STATE_ACTIVATING
        || migration.target_key != fence.target_key
        || migration.target_generation != fence.generation
        || migration.switch_token != fence.switch_token
        || migration.cancel_requested_at.is_some()
        || migration.cancel_idempotency_key_hash.is_some()
        || migration.error_code.is_some()
    {
        return Err(AppError::TenantOperationConflict(
            "迁移投影的 activating 状态或围栏身份已变化".into(),
        ));
    }
    if lease.owner_token != fence.switch_token
        || lease.operation != "tenant_data.migration"
        || lease.resource_type != "tenant_data_migration"
        || lease.resource_id != migration.id.to_string()
    {
        return Err(AppError::TenantOperationConflict(
            "迁移投影操作租约不属于当前迁移".into(),
        ));
    }
    Ok(())
}
