use super::{fence::*, *};

impl TenantDatabaseRouter {
    pub async fn target_occupancy(
        &self,
        target_key: &str,
    ) -> Result<Option<TenantDataTargetOccupancy>, TenantDataError> {
        self.target_occupancy_for_catalog(target_key, &crate::migration::TENANT_DATA_CATALOG)
            .await
    }

    pub async fn target_occupancy_for_catalog(
        &self,
        target_key: &str,
        catalog: &crate::migration::TenantDataCatalog,
    ) -> Result<Option<TenantDataTargetOccupancy>, TenantDataError> {
        let handle = self.open_target_for_catalog(target_key, catalog).await?;
        if handle.mode != TenantDatabaseTargetMode::Dedicated {
            return Ok(None);
        }
        let row = TargetSlotRow::find_by_statement(Statement::from_string(
            DbBackend::MySql,
            TARGET_SLOT_QUERY,
        ))
        .one(handle.connection())
        .await
        .map_err(|error| {
            tracing::warn!(target = target_key, %error, "dedicated 固定占用槽读取失败");
            TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            }
        })?
        .ok_or_else(|| TenantDataError::TargetUnavailable {
            target_key: target_key.into(),
        })?;
        match (row.tenant_id, row.placement_generation, row.switch_token) {
            (None, None, None) => Ok(None),
            (Some(tenant_id), Some(generation), Some(switch_token)) if generation > 0 => {
                Ok(Some(TenantDataTargetOccupancy {
                    tenant_id,
                    placement_generation: generation,
                    switch_token,
                }))
            }
            _ => Err(TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            }),
        }
    }

    /// 按编译期 catalog 检查目标内该租户是否完全为空。
    pub async fn tenant_is_empty_on_target(
        &self,
        target_key: &str,
        tenant_id: &str,
    ) -> Result<bool, TenantDataError> {
        self.tenant_is_empty_on_target_for_catalog(
            target_key,
            tenant_id,
            &crate::migration::TENANT_DATA_CATALOG,
        )
        .await
    }

    pub async fn tenant_is_empty_on_target_for_catalog(
        &self,
        target_key: &str,
        tenant_id: &str,
        catalog: &crate::migration::TenantDataCatalog,
    ) -> Result<bool, TenantDataError> {
        let handle = self.open_target_for_catalog(target_key, catalog).await?;
        let fence = FenceRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::MySql,
            FENCE_QUERY,
            [tenant_id.into()],
        ))
        .one(handle.connection())
        .await
        .map_err(|error| {
            tracing::warn!(target = target_key, %error, "目标租户 fence 空闲检查失败");
            TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            }
        })?;
        if fence.is_some() {
            return Ok(false);
        }
        if handle.mode == TenantDatabaseTargetMode::Dedicated {
            let slot = TargetSlotRow::find_by_statement(Statement::from_string(
                DbBackend::MySql,
                TARGET_SLOT_QUERY,
            ))
            .one(handle.connection())
            .await
            .map_err(|error| {
                tracing::warn!(target = target_key, %error, "dedicated 目标占用槽空闲检查失败");
                TenantDataError::TargetUnavailable {
                    target_key: target_key.into(),
                }
            })?
            .ok_or_else(|| TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            })?;
            if slot.tenant_id.is_some() {
                return Ok(false);
            }
        }
        Ok(!tenant_catalog_rows_exist(handle.connection(), tenant_id, target_key, catalog).await?)
    }

    /// 迁移切换前在目标创建 frozen fence，并为 dedicated 目标原子占用固定槽。
    pub async fn prepare_migration_target(
        &self,
        tenant_id: &str,
        target_key: &str,
        placement_generation: i64,
        switch_token: &str,
    ) -> Result<(), TenantDataError> {
        self.prepare_migration_target_for_catalog(
            tenant_id,
            target_key,
            placement_generation,
            switch_token,
            &crate::migration::TENANT_DATA_CATALOG,
        )
        .await
    }

    /// 在同一目标事务中完成空数据、fence 与 dedicated 槽检查并创建 frozen fence。
    /// 精确 token 的 frozen fence/slot 视为 crash-safe 幂等续跑，其他历史元数据 fail-closed。
    pub async fn prepare_migration_target_for_catalog(
        &self,
        tenant_id: &str,
        target_key: &str,
        placement_generation: i64,
        switch_token: &str,
        catalog: &crate::migration::TenantDataCatalog,
    ) -> Result<(), TenantDataError> {
        let handle = self.open_target_for_catalog(target_key, catalog).await?;
        let provision = self.prepare_fence_provision(
            tenant_id,
            target_key,
            placement_generation,
            switch_token,
        )?;
        let transaction = handle.connection().begin().await.map_err(|error| {
            tracing::warn!(target = target_key, %error, "迁移目标 fence 事务开启失败");
            TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            }
        })?;
        let result =
            prepare_frozen_fence_in_transaction(&transaction, &provision, handle.mode, catalog)
                .await;
        finish_target_transaction(transaction, result, target_key).await
    }
}

async fn prepare_frozen_fence_in_transaction(
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
    let existing = lock_fence(transaction, provision).await?;
    match existing {
        Some(existing) if fence_matches(&existing, provision) && existing.state == "frozen" => {
            require_exact_dedicated_slot(slot.as_ref(), provision)?;
            return Ok(());
        }
        Some(existing) => {
            return Err(TenantDataError::StalePlacementGeneration {
                tenant_id: provision.tenant_id.clone(),
                session_generation: provision.placement_generation,
                current_generation: existing.placement_generation,
            });
        }
        None => {
            if slot.as_ref().is_some_and(|slot| slot.tenant_id.is_some()) {
                return Err(TenantDataError::DedicatedTargetOccupied {
                    target_key: provision.target_key.clone(),
                });
            }
            if tenant_catalog_rows_exist(
                transaction,
                &provision.tenant_id,
                &provision.target_key,
                catalog,
            )
            .await?
            {
                return Err(TenantDataError::FenceRejected {
                    tenant_id: provision.tenant_id.clone(),
                    target_key: provision.target_key.clone(),
                    reason: "目标存在无本迁移所有权的租户数据".into(),
                });
            }
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DbBackend::MySql,
                    "INSERT INTO biz_tenant_fence \
                     (tenant_id, target_key, placement_generation, state, switch_token, updated_at) \
                     VALUES (?, ?, ?, 'frozen', ?, CURRENT_TIMESTAMP(6))",
                    [
                        provision.tenant_id.clone().into(),
                        provision.target_key.clone().into(),
                        provision.placement_generation.into(),
                        provision.switch_token.clone().into(),
                    ],
                ))
                .await
                .map_err(|error| target_write_error(provision, error, "迁移目标 frozen fence 创建失败"))?;
        }
    }
    claim_dedicated_slot(transaction, provision, target_mode).await
}

async fn tenant_catalog_rows_exist<C>(
    connection: &C,
    tenant_id: &str,
    target_key: &str,
    catalog: &crate::migration::TenantDataCatalog,
) -> Result<bool, TenantDataError>
where
    C: ConnectionTrait,
{
    for descriptor in catalog.tables() {
        let row = connection
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                format!(
                    "SELECT EXISTS(SELECT 1 FROM `{}` WHERE `tenant_id` = ? LIMIT 1)",
                    descriptor.table
                ),
                [tenant_id.into()],
            ))
            .await
            .map_err(|error| {
                tracing::warn!(target = target_key, table = descriptor.table, %error, "目标租户空数据检查失败");
                TenantDataError::TargetUnavailable {
                    target_key: target_key.into(),
                }
            })?
            .ok_or_else(|| TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            })?;
        if row.try_get_by_index::<i64>(0).map_err(|error| {
            tracing::warn!(target = target_key, table = descriptor.table, %error, "目标租户空数据结果无效");
            TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            }
        })? != 0
        {
            return Ok(true);
        }
    }
    Ok(false)
}
