use super::*;

pub(super) async fn delete_tenant_rows_batch_in_transaction(
    transaction: &DatabaseTransaction,
    provision: &FenceProvision,
    target_mode: TenantDatabaseTargetMode,
    descriptor: &crate::migration::TenantDataTableDescriptor,
    batch_size: u32,
) -> Result<u64, TenantDataError> {
    let slot = if target_mode == TenantDatabaseTargetMode::Dedicated {
        Some(lock_target_slot(transaction, &provision.target_key).await?)
    } else {
        None
    };
    let Some(fence) = lock_fence(transaction, provision).await? else {
        if slot.as_ref().is_some_and(|slot| slot.tenant_id.is_some()) {
            return Err(TenantDataError::TargetUnavailable {
                target_key: provision.target_key.clone(),
            });
        }
        let row = transaction
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                format!(
                    "SELECT EXISTS(SELECT 1 FROM `{}` WHERE `tenant_id` = ? LIMIT 1)",
                    descriptor.table
                ),
                [provision.tenant_id.clone().into()],
            ))
            .await
            .map_err(|error| target_write_error(provision, error, "批量清理幂等检查失败"))?
            .ok_or_else(|| TenantDataError::TargetUnavailable {
                target_key: provision.target_key.clone(),
            })?;
        if row
            .try_get_by_index::<i64>(0)
            .map_err(|error| target_write_error(provision, error, "批量清理幂等结果无效"))?
            != 0
        {
            return Err(TenantDataError::FenceRejected {
                tenant_id: provision.tenant_id.clone(),
                target_key: provision.target_key.clone(),
                reason: "缺少 migration-owned frozen fence，拒绝删除 catalog 数据".into(),
            });
        }
        return Ok(0);
    };
    require_owned_frozen_fence(&fence, provision)?;
    require_exact_dedicated_slot(slot.as_ref(), provision)?;
    let order = descriptor
        .primary_key_cursor_columns
        .iter()
        .map(|column| format!("`{column}`"))
        .collect::<Vec<_>>()
        .join(", ");
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            format!(
                "DELETE FROM `{}` WHERE `tenant_id` = ? ORDER BY {order} LIMIT {batch_size}",
                descriptor.table
            ),
            [provision.tenant_id.clone().into()],
        ))
        .await
        .map(|result| result.rows_affected())
        .map_err(|error| target_write_error(provision, error, "租户 catalog 数据批量清理失败"))
}

pub(super) async fn cleanup_ownership_in_transaction(
    transaction: &DatabaseTransaction,
    provision: &FenceProvision,
    target_mode: TenantDatabaseTargetMode,
    catalog: &crate::migration::TenantDataCatalog,
) -> Result<TenantDataCleanupOwnership, TenantDataError> {
    let slot = if target_mode == TenantDatabaseTargetMode::Dedicated {
        Some(lock_target_slot(transaction, &provision.target_key).await?)
    } else {
        None
    };
    let fence = lock_fence(transaction, provision).await?;
    if let Some(fence) = &fence {
        if require_owned_frozen_fence(fence, provision).is_err()
            || require_exact_dedicated_slot(slot.as_ref(), provision).is_err()
        {
            return Ok(TenantDataCleanupOwnership::NotOwned);
        }
        return Ok(TenantDataCleanupOwnership::OwnedFrozen);
    }
    if slot.as_ref().is_some_and(|slot| slot.tenant_id.is_some()) {
        return Ok(TenantDataCleanupOwnership::NotOwned);
    }
    for descriptor in catalog.tables() {
        let row = transaction
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                format!(
                    "SELECT EXISTS(SELECT 1 FROM `{}` WHERE `tenant_id` = ? LIMIT 1)",
                    descriptor.table
                ),
                [provision.tenant_id.clone().into()],
            ))
            .await
            .map_err(|error| {
                target_write_error(provision, error, "cleanup 所有权 catalog 检查失败")
            })?
            .ok_or_else(|| TenantDataError::TargetUnavailable {
                target_key: provision.target_key.clone(),
            })?;
        if row.try_get_by_index::<i64>(0).map_err(|error| {
            target_write_error(provision, error, "cleanup 所有权 catalog 结果无效")
        })? != 0
        {
            return Ok(TenantDataCleanupOwnership::NotOwned);
        }
    }
    Ok(TenantDataCleanupOwnership::AlreadyClean)
}

pub(super) async fn finish_tenant_cleanup_in_transaction(
    transaction: &DatabaseTransaction,
    provision: &FenceProvision,
    target_mode: TenantDatabaseTargetMode,
    catalog: &crate::migration::TenantDataCatalog,
) -> Result<(), TenantDataError> {
    let slot = if target_mode == TenantDatabaseTargetMode::Dedicated {
        Some(lock_target_slot(transaction, &provision.target_key).await?)
    } else {
        None
    };
    let fence = lock_fence(transaction, provision).await?;
    if let Some(fence) = &fence {
        require_owned_frozen_fence(fence, provision)?;
        require_exact_dedicated_slot(slot.as_ref(), provision)?;
    } else if slot.as_ref().is_some_and(|slot| slot.tenant_id.is_some()) {
        return Err(TenantDataError::TargetUnavailable {
            target_key: provision.target_key.clone(),
        });
    }
    for descriptor in catalog.tables() {
        let row = transaction
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                format!(
                    "SELECT EXISTS(SELECT 1 FROM `{}` WHERE `tenant_id` = ? LIMIT 1)",
                    descriptor.table
                ),
                [provision.tenant_id.clone().into()],
            ))
            .await
            .map_err(|error| target_write_error(provision, error, "清理收口空数据检查失败"))?
            .ok_or_else(|| TenantDataError::TargetUnavailable {
                target_key: provision.target_key.clone(),
            })?;
        if row
            .try_get_by_index::<i64>(0)
            .map_err(|error| target_write_error(provision, error, "清理收口空数据结果无效"))?
            != 0
        {
            return Err(TenantDataError::FenceRejected {
                tenant_id: provision.tenant_id.clone(),
                target_key: provision.target_key.clone(),
                reason: "catalog 数据尚未清空".into(),
            });
        }
    }
    if fence.is_some() {
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                "DELETE FROM biz_tenant_fence WHERE tenant_id = ? AND target_key = ? \
                 AND placement_generation = ? AND switch_token = ? AND state = 'frozen'",
                [
                    provision.tenant_id.clone().into(),
                    provision.target_key.clone().into(),
                    provision.placement_generation.into(),
                    provision.switch_token.clone().into(),
                ],
            ))
            .await
            .map_err(|error| target_write_error(provision, error, "frozen fence 清理收口失败"))?;
    }
    if slot.is_some() {
        transaction
            .execute_raw(Statement::from_string(
                DbBackend::MySql,
                "UPDATE biz_tenant_target_slot SET tenant_id = NULL, \
                 placement_generation = NULL, switch_token = NULL, \
                 updated_at = CURRENT_TIMESTAMP(6) WHERE slot_id = 1",
            ))
            .await
            .map_err(|error| target_write_error(provision, error, "dedicated 槽清理收口失败"))?;
    }
    Ok(())
}
