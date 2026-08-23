use super::super::*;

pub(in crate::router) async fn provision_fence_in_transaction(
    transaction: &DatabaseTransaction,
    provision: &FenceProvision,
    target_mode: TenantDatabaseTargetMode,
) -> Result<(), TenantDataError> {
    if target_mode == TenantDatabaseTargetMode::Dedicated {
        // 固定单例行锁在 READ COMMITTED/REPEATABLE READ 下都能确定性串行化首次占用。
        let slot = TargetSlotRow::find_by_statement(Statement::from_string(
            DbBackend::MySql,
            TARGET_SLOT_LOCK_QUERY,
        ))
        .one(transaction)
        .await
        .map_err(|error| {
            tracing::warn!(target = %provision.target_key, %error, "dedicated 固定占用槽锁定失败");
            TenantDataError::TargetUnavailable {
                target_key: provision.target_key.clone(),
            }
        })?
        .ok_or_else(|| TenantDataError::TargetUnavailable {
            target_key: provision.target_key.clone(),
        })?;
        if slot
            .tenant_id
            .as_deref()
            .is_some_and(|tenant_id| tenant_id != provision.tenant_id)
        {
            return Err(TenantDataError::DedicatedTargetOccupied {
                target_key: provision.target_key.clone(),
            });
        }
    }

    let existing = FenceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        FENCE_LOCK_QUERY,
        [provision.tenant_id.clone().into()],
    ))
    .one(transaction)
    .await
    .map_err(|error| {
        tracing::warn!(tenant_id = %provision.tenant_id, target = %provision.target_key, %error, "现有 fence 查询失败");
        TenantDataError::TargetUnavailable {
            target_key: provision.target_key.clone(),
        }
    })?;
    if let Some(existing) = existing {
        let exact = existing.tenant_id == provision.tenant_id
            && existing.target_key == provision.target_key
            && existing.placement_generation > 0
            && existing.placement_generation == provision.placement_generation
            && existing.switch_token == provision.switch_token;
        if exact && existing.state == "active" {
            claim_dedicated_slot(transaction, provision, target_mode).await?;
            return Ok(());
        }
        if exact && existing.state == "frozen" {
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DbBackend::MySql,
                    "UPDATE biz_tenant_fence SET state = 'active', updated_at = CURRENT_TIMESTAMP(6) \
                     WHERE tenant_id = ? AND placement_generation = ? AND switch_token = ?",
                    [
                        provision.tenant_id.clone().into(),
                        provision.placement_generation.into(),
                        provision.switch_token.clone().into(),
                    ],
                ))
                .await
                .map_err(|error| {
                    tracing::warn!(tenant_id = %provision.tenant_id, %error, "frozen fence 激活失败");
                    TenantDataError::TargetUnavailable {
                        target_key: provision.target_key.clone(),
                    }
                })?;
            claim_dedicated_slot(transaction, provision, target_mode).await?;
            return Ok(());
        }
        return Err(TenantDataError::StalePlacementGeneration {
            tenant_id: provision.tenant_id.clone(),
            session_generation: provision.placement_generation,
            current_generation: existing.placement_generation,
        });
    }

    transaction
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "INSERT INTO biz_tenant_fence \
             (tenant_id, target_key, placement_generation, state, switch_token, updated_at) \
             VALUES (?, ?, ?, 'active', ?, CURRENT_TIMESTAMP(6))",
            [
                provision.tenant_id.clone().into(),
                provision.target_key.clone().into(),
                provision.placement_generation.into(),
                provision.switch_token.clone().into(),
            ],
        ))
        .await
        .map_err(|error| {
            tracing::warn!(tenant_id = %provision.tenant_id, target = %provision.target_key, %error, "初始 active fence 创建失败");
            TenantDataError::TargetUnavailable {
                target_key: provision.target_key.clone(),
            }
        })?;
    claim_dedicated_slot(transaction, provision, target_mode).await?;
    Ok(())
}

pub(in crate::router) async fn set_fence_state_in_transaction(
    transaction: &DatabaseTransaction,
    provision: &FenceProvision,
    target_mode: TenantDatabaseTargetMode,
    expected_state: &str,
    target_state: &str,
) -> Result<(), TenantDataError> {
    if target_mode == TenantDatabaseTargetMode::Dedicated {
        let slot = lock_target_slot(transaction, &provision.target_key).await?;
        let exact = slot.tenant_id.as_deref() == Some(provision.tenant_id.as_str())
            && slot.placement_generation == Some(provision.placement_generation)
            && slot.switch_token.as_deref() == Some(provision.switch_token.as_str());
        if !exact {
            return Err(TenantDataError::DedicatedTargetOccupied {
                target_key: provision.target_key.clone(),
            });
        }
    }
    let existing = lock_fence(transaction, provision).await?.ok_or_else(|| {
        TenantDataError::FenceRejected {
            tenant_id: provision.tenant_id.clone(),
            target_key: provision.target_key.clone(),
            reason: "fence 不存在".into(),
        }
    })?;
    if !fence_matches(&existing, provision) {
        return Err(TenantDataError::StalePlacementGeneration {
            tenant_id: provision.tenant_id.clone(),
            session_generation: provision.placement_generation,
            current_generation: existing.placement_generation,
        });
    }
    if existing.state == target_state {
        return Ok(());
    }
    if existing.state != expected_state {
        return Err(TenantDataError::FenceRejected {
            tenant_id: provision.tenant_id.clone(),
            target_key: provision.target_key.clone(),
            reason: "fence 状态不允许当前转换".into(),
        });
    }
    let result = transaction
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "UPDATE biz_tenant_fence SET state = ?, updated_at = CURRENT_TIMESTAMP(6) \
             WHERE tenant_id = ? AND target_key = ? AND placement_generation = ? \
               AND switch_token = ? AND state = ?",
            [
                target_state.into(),
                provision.tenant_id.clone().into(),
                provision.target_key.clone().into(),
                provision.placement_generation.into(),
                provision.switch_token.clone().into(),
                expected_state.into(),
            ],
        ))
        .await
        .map_err(|error| target_write_error(provision, error, "fence 状态转换失败"))?;
    if result.rows_affected() > 1 {
        return Err(TenantDataError::TargetUnavailable {
            target_key: provision.target_key.clone(),
        });
    }
    Ok(())
}

pub(in crate::router) fn require_owned_frozen_fence(
    fence: &FenceRow,
    provision: &FenceProvision,
) -> Result<(), TenantDataError> {
    if !fence_matches(fence, provision) {
        return Err(TenantDataError::StalePlacementGeneration {
            tenant_id: provision.tenant_id.clone(),
            session_generation: provision.placement_generation,
            current_generation: fence.placement_generation,
        });
    }
    if fence.state != "frozen" {
        return Err(TenantDataError::FenceRejected {
            tenant_id: provision.tenant_id.clone(),
            target_key: provision.target_key.clone(),
            reason: "仅允许清理 migration-owned frozen fence".into(),
        });
    }
    Ok(())
}

pub(in crate::router) fn require_exact_dedicated_slot(
    slot: Option<&TargetSlotRow>,
    provision: &FenceProvision,
) -> Result<(), TenantDataError> {
    let Some(slot) = slot else {
        return Ok(());
    };
    if slot.tenant_id.as_deref() == Some(provision.tenant_id.as_str())
        && slot.placement_generation == Some(provision.placement_generation)
        && slot.switch_token.as_deref() == Some(provision.switch_token.as_str())
    {
        Ok(())
    } else {
        Err(TenantDataError::DedicatedTargetOccupied {
            target_key: provision.target_key.clone(),
        })
    }
}

pub(in crate::router) async fn lock_target_slot(
    transaction: &DatabaseTransaction,
    target_key: &str,
) -> Result<TargetSlotRow, TenantDataError> {
    TargetSlotRow::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        TARGET_SLOT_LOCK_QUERY,
    ))
    .one(transaction)
    .await
    .map_err(|error| {
        tracing::warn!(target = target_key, %error, "dedicated 固定占用槽锁定失败");
        TenantDataError::TargetUnavailable {
            target_key: target_key.into(),
        }
    })?
    .ok_or_else(|| TenantDataError::TargetUnavailable {
        target_key: target_key.into(),
    })
}

pub(in crate::router) async fn lock_fence(
    transaction: &DatabaseTransaction,
    provision: &FenceProvision,
) -> Result<Option<FenceRow>, TenantDataError> {
    FenceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        FENCE_LOCK_QUERY,
        [provision.tenant_id.clone().into()],
    ))
    .one(transaction)
    .await
    .map_err(|error| target_write_error(provision, error, "fence 锁定失败"))
}

pub(in crate::router) fn fence_matches(fence: &FenceRow, provision: &FenceProvision) -> bool {
    fence.tenant_id == provision.tenant_id
        && fence.target_key == provision.target_key
        && fence.placement_generation > 0
        && fence.placement_generation == provision.placement_generation
        && fence.switch_token == provision.switch_token
}

pub(in crate::router) fn target_write_error(
    provision: &FenceProvision,
    error: impl std::fmt::Display,
    operation: &'static str,
) -> TenantDataError {
    tracing::warn!(tenant_id = %provision.tenant_id, target = %provision.target_key, %error, operation);
    TenantDataError::TargetUnavailable {
        target_key: provision.target_key.clone(),
    }
}

pub(in crate::router) async fn finish_target_transaction(
    transaction: DatabaseTransaction,
    result: Result<(), TenantDataError>,
    target_key: &str,
) -> Result<(), TenantDataError> {
    match result {
        Ok(()) => transaction.commit().await.map_err(|error| {
            tracing::warn!(target = target_key, %error, "租户数据目标事务提交失败");
            TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            }
        }),
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

pub(in crate::router) async fn claim_dedicated_slot(
    transaction: &DatabaseTransaction,
    provision: &FenceProvision,
    target_mode: TenantDatabaseTargetMode,
) -> Result<(), TenantDataError> {
    if target_mode != TenantDatabaseTargetMode::Dedicated {
        return Ok(());
    }
    let result = transaction
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "UPDATE biz_tenant_target_slot SET tenant_id = ?, placement_generation = ?, \
             switch_token = ?, updated_at = CURRENT_TIMESTAMP(6) WHERE slot_id = 1",
            [
                provision.tenant_id.clone().into(),
                provision.placement_generation.into(),
                provision.switch_token.clone().into(),
            ],
        ))
        .await
        .map_err(|error| {
            tracing::warn!(target = %provision.target_key, %error, "dedicated 固定占用槽更新失败");
            TenantDataError::TargetUnavailable {
                target_key: provision.target_key.clone(),
            }
        })?;
    if result.rows_affected() > 1 {
        return Err(TenantDataError::TargetUnavailable {
            target_key: provision.target_key.clone(),
        });
    }
    if result.rows_affected() == 0 {
        // MySQL 默认按 changed rows 计数；幂等写入相同值可能返回 0。
        let slot = TargetSlotRow::find_by_statement(Statement::from_string(
            DbBackend::MySql,
            TARGET_SLOT_LOCK_QUERY,
        ))
        .one(transaction)
        .await
        .map_err(|error| {
            tracing::warn!(target = %provision.target_key, %error, "dedicated 固定占用槽复核失败");
            TenantDataError::TargetUnavailable {
                target_key: provision.target_key.clone(),
            }
        })?;
        let exact = slot.is_some_and(|slot| {
            slot.tenant_id.as_deref() == Some(provision.tenant_id.as_str())
                && slot.placement_generation == Some(provision.placement_generation)
                && slot.switch_token.as_deref() == Some(provision.switch_token.as_str())
        });
        if !exact {
            return Err(TenantDataError::TargetUnavailable {
                target_key: provision.target_key.clone(),
            });
        }
    }
    Ok(())
}
