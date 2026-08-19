use super::*;

impl TenantDatabaseRouter {
    pub async fn runtime_snapshot(
        &self,
        tenant_id: &str,
    ) -> Result<TenantRuntimeSnapshot, TenantDataError> {
        ryframe_kernel::TenantId::parse(tenant_id)
            .map_err(|error| TenantDataError::InvalidTenantId(error.message().into()))?;
        let row = RuntimeSnapshotRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::MySql,
            RUNTIME_SNAPSHOT_QUERY,
            [tenant_id.into()],
        ))
        .one(self.inner.control.write())
        .await
        .map_err(|error| {
            tracing::warn!(%tenant_id, %error, "租户运行时快照查询失败");
            TenantDataError::PlacementUnavailable {
                tenant_id: tenant_id.into(),
            }
        })?
        .ok_or_else(|| TenantDataError::PlacementUnavailable {
            tenant_id: tenant_id.into(),
        })?;
        if row.tenant_id != tenant_id
            || row.authorization_epoch <= 0
            || row.runtime_epoch <= 0
            || row.placement_generation <= 0
        {
            return Err(TenantDataError::InvalidPlacement {
                tenant_id: tenant_id.into(),
                reason: "authorization_epoch、runtime_epoch 或 placement_generation 无效".into(),
            });
        }
        let business_data_state =
            TenantDataState::parse(&row.business_data_state).ok_or_else(|| {
                TenantDataError::InvalidPlacement {
                    tenant_id: tenant_id.into(),
                    reason: "未知 business_data state".into(),
                }
            })?;
        Ok(TenantRuntimeSnapshot::new(
            row.tenant_id,
            row.authorization_epoch as u64,
            row.runtime_epoch as u64,
            row.placement_generation,
            business_data_state,
        ))
    }
    pub async fn resolve(&self, tenant_id: &str) -> Result<TenantDataSession, TenantDataError> {
        ryframe_kernel::TenantId::parse(tenant_id)
            .map_err(|error| TenantDataError::InvalidTenantId(error.message().into()))?;
        let placement = self.load_placement(tenant_id).await?;
        let target_mode = self
            .inner
            .targets
            .target_mode(placement.target_key())
            .ok_or_else(|| TenantDataError::UnknownTarget {
                target_key: placement.target_key().into(),
            })?;
        let database = self
            .verified_database_for_target(placement.target_key())
            .await?;
        let session = TenantDataSession {
            placement,
            target_mode,
            database,
        };
        session.verify_target_fence(TenantDataAccess::Read).await?;
        Ok(session)
    }
    async fn load_placement(
        &self,
        tenant_id: &str,
    ) -> Result<TenantDataPlacement, TenantDataError> {
        let row = PlacementRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::MySql,
            PLACEMENT_QUERY,
            [tenant_id.into()],
        ))
        .one(self.inner.control.write())
        .await
        .map_err(|error| {
            tracing::warn!(%tenant_id, %error, "租户权威数据 placement 查询失败");
            TenantDataError::PlacementUnavailable {
                tenant_id: tenant_id.into(),
            }
        })?
        .ok_or_else(|| TenantDataError::PlacementUnavailable {
            tenant_id: tenant_id.into(),
        })?;
        if row.tenant_id != tenant_id || row.placement_generation <= 0 {
            return Err(TenantDataError::InvalidPlacement {
                tenant_id: tenant_id.into(),
                reason: "tenant_id 或 placement_generation 无效".into(),
            });
        }
        let state = TenantDataState::parse(&row.state).ok_or_else(|| {
            TenantDataError::InvalidPlacement {
                tenant_id: tenant_id.into(),
                reason: "未知 placement state".into(),
            }
        })?;
        if state != TenantDataState::Active {
            return Err(TenantDataError::TenantDataMaintenance {
                tenant_id: tenant_id.into(),
                generation: row.placement_generation,
            });
        }
        TenantDataPlacement::new(
            row.tenant_id,
            row.current_target_key,
            row.placement_generation,
            row.switch_token,
            state,
        )
    }
}
