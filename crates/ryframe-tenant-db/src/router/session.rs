use super::connection::SessionDatabase;
use super::*;

impl TenantDataSession {
    pub fn tenant_id(&self) -> &str {
        self.placement.tenant_id()
    }

    pub fn target_key(&self) -> &str {
        self.placement.target_key()
    }

    pub const fn placement_generation(&self) -> i64 {
        self.placement.generation()
    }

    pub const fn data_state(&self) -> TenantDataState {
        self.placement.state()
    }

    pub fn is_shared_control(&self) -> bool {
        matches!(self.database, SessionDatabase::SharedControl(_))
    }

    /// 先在目标 writer 校验 fence，再在同一目标内部选择读取连接。
    pub async fn select_read(
        &self,
        consistency: ReadConsistency,
    ) -> Result<SelectedDatabase, TenantDataError> {
        self.verify_target_fence(TenantDataAccess::Read).await?;
        Ok(match &self.database {
            SessionDatabase::SharedControl(control) => control.select_read(consistency),
            SessionDatabase::Mysql(lease) => SelectedDatabase {
                node_name: Arc::from(self.placement.target_key()),
                kind: DatabaseNodeKind::Primary,
                connection: lease.connection().clone(),
            },
        })
    }

    /// 开启写事务并首先锁定 fence；锁由返回事务一直持有到调用方 commit/rollback。
    pub async fn begin_write(&self) -> Result<DatabaseTransaction, TenantDataError> {
        let transaction = self.database.writer().begin().await.map_err(|error| {
            tracing::warn!(
                target = self.target_key(),
                tenant_id = self.tenant_id(),
                %error,
                "租户数据写事务开启失败"
            );
            TenantDataError::TargetUnavailable {
                target_key: self.target_key().into(),
            }
        })?;
        if let Err(error) = self
            .verify_fence_on(&transaction, TenantDataAccess::Write, true)
            .await
        {
            let _ = transaction.rollback().await;
            return Err(error);
        }
        Ok(transaction)
    }

    pub(super) async fn verify_target_fence(
        &self,
        access: TenantDataAccess,
    ) -> Result<(), TenantDataError> {
        self.verify_fence_on(self.database.writer(), access, false)
            .await
    }

    async fn verify_fence_on<C>(
        &self,
        connection: &C,
        _access: TenantDataAccess,
        lock: bool,
    ) -> Result<(), TenantDataError>
    where
        C: ConnectionTrait,
    {
        let target_slot = if self.target_mode == TenantDatabaseTargetMode::Dedicated {
            let sql = if lock {
                TARGET_SLOT_LOCK_QUERY
            } else {
                TARGET_SLOT_QUERY
            };
            Some(
                TargetSlotRow::find_by_statement(Statement::from_string(DbBackend::MySql, sql))
                    .one(connection)
                    .await
                    .map_err(|error| {
                        tracing::warn!(
                            target = self.target_key(),
                            tenant_id = self.tenant_id(),
                            %error,
                            "dedicated 固定占用槽检查失败"
                        );
                        TenantDataError::TargetUnavailable {
                            target_key: self.target_key().into(),
                        }
                    })?
                    .ok_or_else(|| TenantDataError::TargetUnavailable {
                        target_key: self.target_key().into(),
                    })?,
            )
        } else {
            None
        };
        let sql = if lock { FENCE_LOCK_QUERY } else { FENCE_QUERY };
        let row = FenceRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::MySql,
            sql,
            [self.tenant_id().into()],
        ))
        .one(connection)
        .await
        .map_err(|error| {
            tracing::warn!(
                target = self.target_key(),
                tenant_id = self.tenant_id(),
                %error,
                "租户数据 fence 查询失败"
            );
            TenantDataError::TargetUnavailable {
                target_key: self.target_key().into(),
            }
        })?
        .ok_or_else(|| TenantDataError::FenceRejected {
            tenant_id: self.tenant_id().into(),
            target_key: self.target_key().into(),
            reason: "fence 不存在".into(),
        })?;

        if row.tenant_id != self.tenant_id()
            || row.target_key != self.target_key()
            || row.placement_generation <= 0
            || row.placement_generation != self.placement_generation()
            || row.switch_token != self.placement.switch_token()
        {
            return Err(TenantDataError::StalePlacementGeneration {
                tenant_id: self.tenant_id().into(),
                session_generation: self.placement_generation(),
                current_generation: row.placement_generation,
            });
        }
        if row.state != "active" || row.switch_token.trim().is_empty() {
            return Err(TenantDataError::TenantDataMaintenance {
                tenant_id: self.tenant_id().into(),
                generation: self.placement_generation(),
            });
        }
        if let Some(slot) = target_slot
            && (slot.tenant_id.as_deref() != Some(self.tenant_id())
                || slot.placement_generation != Some(self.placement_generation())
                || slot.switch_token.as_deref() != Some(self.placement.switch_token()))
        {
            return Err(TenantDataError::DedicatedTargetOccupied {
                target_key: self.target_key().into(),
            });
        }
        Ok(())
    }
}
