use super::*;

impl TenantDatabaseRouter {
    pub async fn freeze_fence(
        &self,
        tenant_id: &str,
        target_key: &str,
        placement_generation: i64,
        switch_token: &str,
    ) -> Result<(), TenantDataError> {
        self.set_fence_state(
            tenant_id,
            target_key,
            placement_generation,
            switch_token,
            "active",
            "frozen",
        )
        .await
    }

    pub async fn freeze_fence_for_catalog(
        &self,
        tenant_id: &str,
        target_key: &str,
        placement_generation: i64,
        switch_token: &str,
        catalog: &crate::migration::TenantDataCatalog,
    ) -> Result<(), TenantDataError> {
        let handle = self.open_target_for_catalog(target_key, catalog).await?;
        self.set_fence_state_with_handle(
            handle,
            tenant_id,
            target_key,
            placement_generation,
            switch_token,
            "active",
            "frozen",
        )
        .await
    }

    pub async fn activate_fence(
        &self,
        tenant_id: &str,
        target_key: &str,
        placement_generation: i64,
        switch_token: &str,
    ) -> Result<(), TenantDataError> {
        self.set_fence_state(
            tenant_id,
            target_key,
            placement_generation,
            switch_token,
            "frozen",
            "active",
        )
        .await
    }

    pub async fn activate_fence_for_catalog(
        &self,
        tenant_id: &str,
        target_key: &str,
        placement_generation: i64,
        switch_token: &str,
        catalog: &crate::migration::TenantDataCatalog,
    ) -> Result<(), TenantDataError> {
        let handle = self.open_target_for_catalog(target_key, catalog).await?;
        self.set_fence_state_with_handle(
            handle,
            tenant_id,
            target_key,
            placement_generation,
            switch_token,
            "frozen",
            "active",
        )
        .await
    }

    /// 强一致断言某一 migration 代际仍持有 frozen fence；不改变状态。
    pub async fn assert_frozen_fence_for_catalog(
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
        let fence = FenceRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::MySql,
            FENCE_QUERY,
            [tenant_id.into()],
        ))
        .one(handle.connection())
        .await
        .map_err(|error| target_write_error(&provision, error, "frozen fence 断言失败"))?
        .ok_or_else(|| TenantDataError::FenceRejected {
            tenant_id: tenant_id.into(),
            target_key: target_key.into(),
            reason: "fence 不存在".into(),
        })?;
        require_owned_frozen_fence(&fence, &provision)?;
        if handle.mode == TenantDatabaseTargetMode::Dedicated {
            let slot = TargetSlotRow::find_by_statement(Statement::from_string(
                DbBackend::MySql,
                TARGET_SLOT_QUERY,
            ))
            .one(handle.connection())
            .await
            .map_err(|error| target_write_error(&provision, error, "dedicated slot 断言失败"))?
            .ok_or_else(|| TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            })?;
            require_exact_dedicated_slot(Some(&slot), &provision)?;
        }
        Ok(())
    }

    async fn set_fence_state(
        &self,
        tenant_id: &str,
        target_key: &str,
        placement_generation: i64,
        switch_token: &str,
        expected_state: &str,
        target_state: &str,
    ) -> Result<(), TenantDataError> {
        let handle = self.open_target(target_key).await?;
        self.set_fence_state_with_handle(
            handle,
            tenant_id,
            target_key,
            placement_generation,
            switch_token,
            expected_state,
            target_state,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn set_fence_state_with_handle(
        &self,
        handle: TenantDataTargetHandle,
        tenant_id: &str,
        target_key: &str,
        placement_generation: i64,
        switch_token: &str,
        expected_state: &str,
        target_state: &str,
    ) -> Result<(), TenantDataError> {
        let provision = self.prepare_fence_provision(
            tenant_id,
            target_key,
            placement_generation,
            switch_token,
        )?;
        let transaction = handle.connection().begin().await.map_err(|error| {
            tracing::warn!(target = target_key, %error, "fence 状态事务开启失败");
            TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            }
        })?;
        let result = set_fence_state_in_transaction(
            &transaction,
            &provision,
            handle.mode,
            expected_state,
            target_state,
        )
        .await;
        finish_target_transaction(transaction, result, target_key).await
    }

    /// 切换前取消或失败补偿：反向 FK 顺序清除目标租户数据及 frozen fence。
    pub fn prepare_provisioning(
        &self,
        tenant_id: impl Into<String>,
        target_key: impl Into<String>,
        placement_generation: i64,
        switch_token: impl Into<String>,
    ) -> Result<PendingTenantDataPlacement, TenantDataError> {
        let placement = PendingTenantDataPlacement::new(
            tenant_id,
            target_key,
            placement_generation,
            switch_token,
        )?;
        if !self.inner.targets.contains(&placement.current_target_key) {
            return Err(TenantDataError::UnknownTarget {
                target_key: placement.current_target_key,
            });
        }
        Ok(placement)
    }

    /// 在租户创建 Saga 中幂等 provision 初始 active fence。
    ///
    /// dedicated 目标会在同一事务内锁住 active fence 范围并拒绝第二个活动租户。
    pub async fn provision_tenant_fence(
        &self,
        tenant_id: &str,
        target_key: &str,
        placement_generation: i64,
        switch_token: &str,
    ) -> Result<(), TenantDataError> {
        let provision = self.prepare_fence_provision(
            tenant_id,
            target_key,
            placement_generation,
            switch_token,
        )?;
        self.provision_fence(provision).await
    }

    /// 使用控制库中已持久化的 Saga 输入 provision fence，避免调用方重复拆字段。
    pub async fn provision_pending_fence(
        &self,
        pending: &PendingTenantDataPlacement,
    ) -> Result<(), TenantDataError> {
        let provision = self.prepare_fence_provision(
            &pending.tenant_id,
            &pending.current_target_key,
            pending.placement_generation,
            &pending.switch_token,
        )?;
        self.provision_fence(provision).await
    }

    pub(super) fn prepare_fence_provision(
        &self,
        tenant_id: &str,
        target_key: &str,
        placement_generation: i64,
        switch_token: &str,
    ) -> Result<FenceProvision, TenantDataError> {
        ryframe_kernel::TenantId::parse(tenant_id)
            .map_err(|error| TenantDataError::InvalidTenantId(error.message().into()))?;
        if target_key.trim().is_empty()
            || placement_generation <= 0
            || switch_token.trim().is_empty()
            || switch_token.len() > 64
        {
            return Err(TenantDataError::InvalidPlacement {
                tenant_id: tenant_id.into(),
                reason: "target、generation 或 switch_token 无效".into(),
            });
        }
        if !self.inner.targets.contains(target_key) {
            return Err(TenantDataError::UnknownTarget {
                target_key: target_key.into(),
            });
        }
        Ok(FenceProvision {
            tenant_id: tenant_id.into(),
            target_key: target_key.into(),
            placement_generation,
            switch_token: switch_token.into(),
        })
    }

    async fn provision_fence(&self, provision: FenceProvision) -> Result<(), TenantDataError> {
        let tenant_id = provision.tenant_id.as_str();
        let target_key = provision.target_key.as_str();
        let target_mode = self.inner.targets.target_mode(target_key).ok_or_else(|| {
            TenantDataError::UnknownTarget {
                target_key: target_key.into(),
            }
        })?;
        let database = self.verified_database_for_target(target_key).await?;
        let transaction = database.writer().begin().await.map_err(|error| {
            tracing::warn!(%tenant_id, target = target_key, %error, "fence provision 事务开启失败");
            TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            }
        })?;

        let result = provision_fence_in_transaction(&transaction, &provision, target_mode).await;
        match result {
            Ok(()) => transaction.commit().await.map_err(|error| {
                tracing::warn!(%tenant_id, target = target_key, %error, "fence provision 提交失败");
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
}

async fn provision_fence_in_transaction(
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

pub(super) async fn set_fence_state_in_transaction(
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

pub(super) fn require_owned_frozen_fence(
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

pub(super) fn require_exact_dedicated_slot(
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

pub(super) async fn lock_target_slot(
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

pub(super) async fn lock_fence(
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

pub(super) fn fence_matches(fence: &FenceRow, provision: &FenceProvision) -> bool {
    fence.tenant_id == provision.tenant_id
        && fence.target_key == provision.target_key
        && fence.placement_generation > 0
        && fence.placement_generation == provision.placement_generation
        && fence.switch_token == provision.switch_token
}

pub(super) fn target_write_error(
    provision: &FenceProvision,
    error: impl std::fmt::Display,
    operation: &'static str,
) -> TenantDataError {
    tracing::warn!(tenant_id = %provision.tenant_id, target = %provision.target_key, %error, operation);
    TenantDataError::TargetUnavailable {
        target_key: provision.target_key.clone(),
    }
}

pub(super) async fn finish_target_transaction(
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

pub(super) async fn claim_dedicated_slot(
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
