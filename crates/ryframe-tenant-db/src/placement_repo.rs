use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, FromQueryResult, Statement};

use crate::{TenantDataError, TenantDataState};

/// 租户创建 Saga 在控制库事务内持久化的初始 placement。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingTenantDataPlacement {
    pub tenant_id: String,
    pub current_target_key: String,
    pub placement_generation: i64,
    pub switch_token: String,
}

impl PendingTenantDataPlacement {
    pub fn new(
        tenant_id: impl Into<String>,
        current_target_key: impl Into<String>,
        placement_generation: i64,
        switch_token: impl Into<String>,
    ) -> Result<Self, TenantDataError> {
        let tenant_id = tenant_id.into();
        ryframe_core::validate_tenant_identifier(&tenant_id)
            .map_err(|error| TenantDataError::InvalidTenantId(error.message().into()))?;
        let current_target_key = current_target_key.into();
        let switch_token = switch_token.into();
        if current_target_key.trim().is_empty()
            || placement_generation <= 0
            || switch_token.trim().is_empty()
            || switch_token.len() > 64
        {
            return Err(TenantDataError::InvalidPlacement {
                tenant_id,
                reason: "target、generation 或 switch_token 无效".into(),
            });
        }
        Ok(Self {
            tenant_id,
            current_target_key,
            placement_generation,
            switch_token,
        })
    }
}

/// `sys_tenant_data_placement` 的窄控制面仓储。
#[derive(Clone, Copy, Debug, Default)]
pub struct TenantDataPlacementRepository;

#[derive(Debug, FromQueryResult)]
struct PlacementStateRow {
    current_target_key: String,
    placement_generation: i64,
    state: String,
    switch_token: String,
}

impl TenantDataPlacementRepository {
    /// 与租户基本资料在同一控制库事务内创建 provisioning placement。
    pub async fn create_or_resume_pending(
        &self,
        transaction: &DatabaseTransaction,
        placement: &PendingTenantDataPlacement,
    ) -> Result<(), TenantDataError> {
        let result = transaction
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                "INSERT INTO sys_tenant_data_placement \
                 (tenant_id, current_target_key, placement_generation, state, switch_token, \
                  created_at, updated_at) \
                 VALUES (?, ?, ?, 'provisioning', ?, CURRENT_TIMESTAMP(6), CURRENT_TIMESTAMP(6)) \
                 ON DUPLICATE KEY UPDATE tenant_id = VALUES(tenant_id)",
                [
                    placement.tenant_id.clone().into(),
                    placement.current_target_key.clone().into(),
                    placement.placement_generation.into(),
                    placement.switch_token.clone().into(),
                ],
            ))
            .await
            .map_err(|error| {
                tracing::warn!(
                    tenant_id = %placement.tenant_id,
                    target = %placement.current_target_key,
                    %error,
                    "provisioning placement 创建失败"
                );
                TenantDataError::PlacementUnavailable {
                    tenant_id: placement.tenant_id.clone(),
                }
            })?;
        if result.rows_affected() > 1 {
            return Err(TenantDataError::PlacementUnavailable {
                tenant_id: placement.tenant_id.clone(),
            });
        }
        let state = self.load_exact_state(transaction, placement).await?;
        match state {
            TenantDataState::Provisioning | TenantDataState::Active => Ok(()),
            TenantDataState::Failed => {
                let result = transaction
                    .execute_raw(Statement::from_sql_and_values(
                        DbBackend::MySql,
                        "UPDATE sys_tenant_data_placement SET state = 'provisioning', \
                         updated_at = CURRENT_TIMESTAMP(6) WHERE tenant_id = ? AND state = 'failed'",
                        [placement.tenant_id.clone().into()],
                    ))
                    .await
                    .map_err(|error| {
                        tracing::warn!(tenant_id = %placement.tenant_id, %error, "failed placement 恢复失败");
                        TenantDataError::PlacementUnavailable {
                            tenant_id: placement.tenant_id.clone(),
                        }
                    })?;
                if result.rows_affected() == 1 {
                    Ok(())
                } else {
                    Err(TenantDataError::PlacementUnavailable {
                        tenant_id: placement.tenant_id.clone(),
                    })
                }
            }
            TenantDataState::Maintenance => Err(TenantDataError::TenantDataMaintenance {
                tenant_id: placement.tenant_id.clone(),
                generation: placement.placement_generation,
            }),
        }
    }

    /// 与 `create_or_resume_pending` 同义，供租户创建事务保持窄调用面。
    pub async fn create_pending(
        &self,
        transaction: &DatabaseTransaction,
        placement: &PendingTenantDataPlacement,
    ) -> Result<(), TenantDataError> {
        self.create_or_resume_pending(transaction, placement).await
    }

    async fn load_exact_state<C>(
        &self,
        connection: &C,
        placement: &PendingTenantDataPlacement,
    ) -> Result<TenantDataState, TenantDataError>
    where
        C: ConnectionTrait,
    {
        let row = PlacementStateRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT current_target_key, placement_generation, state, switch_token \
             FROM sys_tenant_data_placement WHERE tenant_id = ? LIMIT 1 FOR UPDATE",
            [placement.tenant_id.clone().into()],
        ))
        .one(connection)
        .await
        .map_err(|error| {
            tracing::warn!(tenant_id = %placement.tenant_id, %error, "placement 幂等状态查询失败");
            TenantDataError::PlacementUnavailable {
                tenant_id: placement.tenant_id.clone(),
            }
        })?
        .ok_or_else(|| TenantDataError::PlacementUnavailable {
            tenant_id: placement.tenant_id.clone(),
        })?;
        let state = TenantDataState::parse(&row.state).ok_or_else(|| {
            TenantDataError::InvalidPlacement {
                tenant_id: placement.tenant_id.clone(),
                reason: "未知 placement state".into(),
            }
        })?;
        if row.current_target_key == placement.current_target_key
            && row.placement_generation > 0
            && row.placement_generation == placement.placement_generation
            && row.switch_token == placement.switch_token
        {
            return Ok(state);
        }
        Err(TenantDataError::StalePlacementGeneration {
            tenant_id: placement.tenant_id.clone(),
            session_generation: placement.placement_generation,
            current_generation: row.placement_generation,
        })
    }

    async fn verify_exact_state<C>(
        &self,
        connection: &C,
        placement: &PendingTenantDataPlacement,
        allowed_states: &[TenantDataState],
    ) -> Result<(), TenantDataError>
    where
        C: ConnectionTrait,
    {
        let state = self.load_exact_state(connection, placement).await?;
        if allowed_states.contains(&state) {
            return Ok(());
        }
        Err(TenantDataError::StalePlacementGeneration {
            tenant_id: placement.tenant_id.clone(),
            session_generation: placement.placement_generation,
            current_generation: placement.placement_generation,
        })
    }

    /// fence 已成功 provision 后，把同一 generation 的 placement 切为 active。
    pub async fn activate<C>(
        &self,
        connection: &C,
        placement: &PendingTenantDataPlacement,
    ) -> Result<(), TenantDataError>
    where
        C: ConnectionTrait,
    {
        self.transition(connection, placement, TenantDataState::Active)
            .await
    }

    /// Saga 补偿路径把尚未 active 的 placement 标记为 failed。
    pub async fn fail<C>(
        &self,
        connection: &C,
        placement: &PendingTenantDataPlacement,
    ) -> Result<(), TenantDataError>
    where
        C: ConnectionTrait,
    {
        self.transition(connection, placement, TenantDataState::Failed)
            .await
    }

    async fn transition<C>(
        &self,
        connection: &C,
        placement: &PendingTenantDataPlacement,
        state: TenantDataState,
    ) -> Result<(), TenantDataError>
    where
        C: ConnectionTrait,
    {
        let result = connection
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                "UPDATE sys_tenant_data_placement SET state = ?, updated_at = CURRENT_TIMESTAMP(6) \
                 WHERE tenant_id = ? AND current_target_key = ? AND placement_generation = ? \
                 AND state = 'provisioning'",
                [
                    state.as_str().into(),
                    placement.tenant_id.clone().into(),
                    placement.current_target_key.clone().into(),
                    placement.placement_generation.into(),
                ],
            ))
            .await
            .map_err(|error| {
                tracing::warn!(
                    tenant_id = %placement.tenant_id,
                    target = %placement.current_target_key,
                    next_state = state.as_str(),
                    %error,
                    "placement 状态转换失败"
                );
                TenantDataError::PlacementUnavailable {
                    tenant_id: placement.tenant_id.clone(),
                }
            })?;
        if result.rows_affected() == 1 {
            return Ok(());
        }
        self.verify_exact_state(connection, placement, &[state])
            .await
    }
}
