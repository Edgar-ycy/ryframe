use super::{fence::*, *};

impl TenantDatabaseRouter {
    pub async fn clear_prepared_target(
        &self,
        tenant_id: &str,
        target_key: &str,
        placement_generation: i64,
        switch_token: &str,
    ) -> Result<(), TenantDataError> {
        self.clear_prepared_target_for_catalog(
            tenant_id,
            target_key,
            placement_generation,
            switch_token,
            &ryframe_tenant_db_migration::TENANT_DATA_CATALOG,
        )
        .await
    }

    pub async fn clear_prepared_target_for_catalog(
        &self,
        tenant_id: &str,
        target_key: &str,
        placement_generation: i64,
        switch_token: &str,
        catalog: &ryframe_tenant_db_migration::TenantDataCatalog,
    ) -> Result<(), TenantDataError> {
        let handle = self.open_target_for_catalog(target_key, catalog).await?;
        let provision = self.prepare_fence_provision(
            tenant_id,
            target_key,
            placement_generation,
            switch_token,
        )?;
        let transaction = handle.connection().begin().await.map_err(|error| {
            tracing::warn!(target = target_key, %error, "迁移目标清理事务开启失败");
            TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            }
        })?;
        let result =
            clear_tenant_data_in_transaction(&transaction, &provision, handle.mode, true, catalog)
                .await;
        finish_target_transaction(transaction, result, target_key).await
    }

    /// 在不删数据的前提下判定 cleanup 所有权，供 Worker 区分
    /// “prepare 前本迁移无副作用”与“已拥有 exact frozen fence”。
    pub async fn cleanup_ownership_for_catalog(
        &self,
        tenant_id: &str,
        target_key: &str,
        placement_generation: i64,
        switch_token: &str,
        catalog: &ryframe_tenant_db_migration::TenantDataCatalog,
    ) -> Result<TenantDataCleanupOwnership, TenantDataError> {
        let handle = self.open_target_for_catalog(target_key, catalog).await?;
        let provision = self.prepare_fence_provision(
            tenant_id,
            target_key,
            placement_generation,
            switch_token,
        )?;
        let transaction = handle.connection().begin().await.map_err(|error| {
            tracing::warn!(target = target_key, %error, "cleanup 所有权检查事务开启失败");
            TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            }
        })?;
        let result =
            cleanup_ownership_in_transaction(&transaction, &provision, handle.mode, catalog).await;
        match result {
            Ok(ownership) => {
                transaction.commit().await.map_err(|error| {
                    tracing::warn!(target = target_key, %error, "cleanup 所有权检查提交失败");
                    TenantDataError::TargetUnavailable {
                        target_key: target_key.into(),
                    }
                })?;
                Ok(ownership)
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    /// 仅对 migration-owned exact frozen fence 下的一张 catalog 表执行有界删除。
    ///
    /// 调用方每批后提交 control 检查点与 operation lease 心跳，避免大事务。
    pub async fn delete_tenant_rows_batch(
        &self,
        tenant_id: &str,
        target_key: &str,
        placement_generation: i64,
        switch_token: &str,
        descriptor: &ryframe_tenant_db_migration::TenantDataTableDescriptor,
        batch_size: u32,
    ) -> Result<u64, TenantDataError> {
        self.delete_tenant_rows_batch_for_catalog(
            TenantDataCleanupBatch {
                tenant_id,
                target_key,
                placement_generation,
                switch_token,
                descriptor,
                batch_size,
            },
            &ryframe_tenant_db_migration::TENANT_DATA_CATALOG,
        )
        .await
    }

    pub async fn delete_tenant_rows_batch_for_catalog(
        &self,
        batch: TenantDataCleanupBatch<'_>,
        catalog: &ryframe_tenant_db_migration::TenantDataCatalog,
    ) -> Result<u64, TenantDataError> {
        let TenantDataCleanupBatch {
            tenant_id,
            target_key,
            placement_generation,
            switch_token,
            descriptor,
            batch_size,
        } = batch;
        if batch_size == 0 || batch_size > 10_000 {
            return Err(TenantDataError::InvalidConfiguration(
                "tenant-data cleanup batch_size 必须为 1..=10000".into(),
            ));
        }
        let handle = self.open_target_for_catalog(target_key, catalog).await?;
        let provision = self.prepare_fence_provision(
            tenant_id,
            target_key,
            placement_generation,
            switch_token,
        )?;
        let transaction = handle.connection().begin().await.map_err(|error| {
            tracing::warn!(target = target_key, %error, "租户数据批量清理事务开启失败");
            TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            }
        })?;
        let result = delete_tenant_rows_batch_in_transaction(
            &transaction,
            &provision,
            handle.mode,
            descriptor,
            batch_size,
        )
        .await;
        match result {
            Ok(rows) => {
                transaction.commit().await.map_err(|error| {
                    tracing::warn!(target = target_key, %error, "租户数据批量清理提交失败");
                    TenantDataError::TargetUnavailable {
                        target_key: target_key.into(),
                    }
                })?;
                Ok(rows)
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    /// 所有 catalog 行均为空后，原子删除 exact frozen fence 并释放 dedicated 槽。
    pub async fn finish_tenant_cleanup_for_catalog(
        &self,
        tenant_id: &str,
        target_key: &str,
        placement_generation: i64,
        switch_token: &str,
        catalog: &ryframe_tenant_db_migration::TenantDataCatalog,
    ) -> Result<(), TenantDataError> {
        let handle = self.open_target_for_catalog(target_key, catalog).await?;
        let provision = self.prepare_fence_provision(
            tenant_id,
            target_key,
            placement_generation,
            switch_token,
        )?;
        let transaction = handle.connection().begin().await.map_err(|error| {
            tracing::warn!(target = target_key, %error, "租户数据清理收口事务开启失败");
            TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            }
        })?;
        let result =
            finish_tenant_cleanup_in_transaction(&transaction, &provision, handle.mode, catalog)
                .await;
        finish_target_transaction(transaction, result, target_key).await
    }

    /// 保留期结束后清除旧源；只接受精确 frozen fence，防止删除已重新激活的代际。
    pub async fn finalize_retained_source(
        &self,
        tenant_id: &str,
        target_key: &str,
        placement_generation: i64,
        switch_token: &str,
    ) -> Result<(), TenantDataError> {
        self.finalize_retained_source_for_catalog(
            tenant_id,
            target_key,
            placement_generation,
            switch_token,
            &ryframe_tenant_db_migration::TENANT_DATA_CATALOG,
        )
        .await
    }

    pub async fn finalize_retained_source_for_catalog(
        &self,
        tenant_id: &str,
        target_key: &str,
        placement_generation: i64,
        switch_token: &str,
        catalog: &ryframe_tenant_db_migration::TenantDataCatalog,
    ) -> Result<(), TenantDataError> {
        let handle = self.open_target_for_catalog(target_key, catalog).await?;
        let provision = self.prepare_fence_provision(
            tenant_id,
            target_key,
            placement_generation,
            switch_token,
        )?;
        let transaction = handle.connection().begin().await.map_err(|error| {
            tracing::warn!(target = target_key, %error, "迁移源清理事务开启失败");
            TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            }
        })?;
        let result =
            clear_tenant_data_in_transaction(&transaction, &provision, handle.mode, true, catalog)
                .await;
        finish_target_transaction(transaction, result, target_key).await
    }
}

async fn clear_tenant_data_in_transaction(
    transaction: &DatabaseTransaction,
    provision: &FenceProvision,
    target_mode: TenantDatabaseTargetMode,
    require_frozen: bool,
    catalog: &ryframe_tenant_db_migration::TenantDataCatalog,
) -> Result<(), TenantDataError> {
    let slot = if target_mode == TenantDatabaseTargetMode::Dedicated {
        Some(lock_target_slot(transaction, &provision.target_key).await?)
    } else {
        None
    };
    let existing = lock_fence(transaction, provision).await?;
    if let Some(existing) = &existing {
        if !fence_matches(existing, provision) {
            return Err(TenantDataError::StalePlacementGeneration {
                tenant_id: provision.tenant_id.clone(),
                session_generation: provision.placement_generation,
                current_generation: existing.placement_generation,
            });
        }
        if require_frozen && existing.state != "frozen" {
            return Err(TenantDataError::FenceRejected {
                tenant_id: provision.tenant_id.clone(),
                target_key: provision.target_key.clone(),
                reason: "只允许清理 frozen fence".into(),
            });
        }
    } else {
        // 没有本 migration token 拥有的 exact frozen fence 时绝不删业务行。
        // shared 目标可能已存在其他历史/孤儿数据；仅当 catalog
        // 已全空才能将 fence absent 视为幂等清理完成。
        if slot.as_ref().is_some_and(|slot| slot.tenant_id.is_some()) {
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
                .map_err(|error| {
                    target_write_error(provision, error, "fence absent 数据安全检查失败")
                })?
                .ok_or_else(|| TenantDataError::TargetUnavailable {
                    target_key: provision.target_key.clone(),
                })?;
            let exists = row.try_get_by_index::<i64>(0).map_err(|error| {
                target_write_error(provision, error, "fence absent 数据安全检查无效")
            })?;
            if exists != 0 {
                return Err(TenantDataError::FenceRejected {
                    tenant_id: provision.tenant_id.clone(),
                    target_key: provision.target_key.clone(),
                    reason: "缺少 migration-owned frozen fence，拒绝删除 catalog 数据".into(),
                });
            }
        }
        return Ok(());
    }

    for descriptor in catalog.tables().iter().rev() {
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                format!("DELETE FROM `{}` WHERE `tenant_id` = ?", descriptor.table),
                [provision.tenant_id.clone().into()],
            ))
            .await
            .map_err(|error| target_write_error(provision, error, "租户 catalog 数据清理失败"))?;
    }
    if existing.is_some() {
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
            .map_err(|error| target_write_error(provision, error, "frozen fence 清理失败"))?;
    }
    if let Some(slot) = slot {
        let exact = slot.tenant_id.as_deref() == Some(provision.tenant_id.as_str())
            && slot.placement_generation == Some(provision.placement_generation)
            && slot.switch_token.as_deref() == Some(provision.switch_token.as_str());
        if exact {
            transaction
                .execute_raw(Statement::from_string(
                    DbBackend::MySql,
                    "UPDATE biz_tenant_target_slot SET tenant_id = NULL, \
                     placement_generation = NULL, switch_token = NULL, \
                     updated_at = CURRENT_TIMESTAMP(6) WHERE slot_id = 1",
                ))
                .await
                .map_err(|error| {
                    target_write_error(provision, error, "dedicated 固定占用槽释放失败")
                })?;
        } else if slot.tenant_id.is_some() {
            return Err(TenantDataError::DedicatedTargetOccupied {
                target_key: provision.target_key.clone(),
            });
        }
    }
    Ok(())
}

async fn delete_tenant_rows_batch_in_transaction(
    transaction: &DatabaseTransaction,
    provision: &FenceProvision,
    target_mode: TenantDatabaseTargetMode,
    descriptor: &ryframe_tenant_db_migration::TenantDataTableDescriptor,
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
    let result = transaction
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            format!(
                "DELETE FROM `{}` WHERE `tenant_id` = ? ORDER BY {order} LIMIT {batch_size}",
                descriptor.table
            ),
            [provision.tenant_id.clone().into()],
        ))
        .await
        .map_err(|error| target_write_error(provision, error, "租户 catalog 数据批量清理失败"))?;
    Ok(result.rows_affected())
}

async fn cleanup_ownership_in_transaction(
    transaction: &DatabaseTransaction,
    provision: &FenceProvision,
    target_mode: TenantDatabaseTargetMode,
    catalog: &ryframe_tenant_db_migration::TenantDataCatalog,
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

async fn finish_tenant_cleanup_in_transaction(
    transaction: &DatabaseTransaction,
    provision: &FenceProvision,
    target_mode: TenantDatabaseTargetMode,
    catalog: &ryframe_tenant_db_migration::TenantDataCatalog,
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
