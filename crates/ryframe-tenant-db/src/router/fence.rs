use super::*;

mod transaction;

use transaction::provision_fence_in_transaction;
pub(super) use transaction::{
    claim_dedicated_slot, fence_matches, finish_target_transaction, lock_fence, lock_target_slot,
    require_exact_dedicated_slot, require_owned_frozen_fence, set_fence_state_in_transaction,
    target_write_error,
};

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
