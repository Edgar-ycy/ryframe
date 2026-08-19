use super::*;

impl TenantDatabaseRouter {
    pub fn new(
        control: ControlDatabaseCluster,
        config: &TenantDataConfig,
        sql_log_level: ryframe_config::SqlLogLevel,
        sql_slow_threshold_ms: u64,
    ) -> Result<Self, TenantDataError> {
        let targets =
            TenantDatabaseTargetRegistry::new(config, sql_log_level, sql_slow_threshold_ms)?;
        Ok(Self::from_registry(control, targets))
    }

    pub fn from_registry(
        control: ControlDatabaseCluster,
        targets: TenantDatabaseTargetRegistry,
    ) -> Self {
        Self {
            inner: Arc::new(RouterInner {
                control,
                targets,
                control_schema_verification: OnceCell::new(),
                control_fresh_verification: tokio::sync::Mutex::new(()),
            }),
        }
    }

    pub fn targets(&self) -> &TenantDatabaseTargetRegistry {
        &self.inner.targets
    }

    /// 仅按已批准 target key 打开并校验目标，永不接受 DSN/连接字段。
    pub async fn open_target(
        &self,
        target_key: &str,
    ) -> Result<TenantDataTargetHandle, TenantDataError> {
        let mode = self.inner.targets.target_mode(target_key).ok_or_else(|| {
            TenantDataError::UnknownTarget {
                target_key: target_key.into(),
            }
        })?;
        let kind = self.inner.targets.target_kind(target_key).ok_or_else(|| {
            TenantDataError::UnknownTarget {
                target_key: target_key.into(),
            }
        })?;
        let database = self.verified_database_for_target(target_key).await?;
        Ok(TenantDataTargetHandle {
            target_key: target_key.into(),
            mode,
            kind,
            database,
        })
    }

    /// 迁移/集成测试专用的 catalog 注入入口。每次执行实时 ping 与完整精确结构校验，
    /// 因此不会把生产空 catalog 的 OnceCell 错当作测试 catalog 证明。
    pub async fn open_target_for_catalog(
        &self,
        target_key: &str,
        catalog: &ryframe_tenant_db_migration::TenantDataCatalog,
    ) -> Result<TenantDataTargetHandle, TenantDataError> {
        if catalog.schema_fingerprint()
            == ryframe_tenant_db_migration::TENANT_DATA_SCHEMA_FINGERPRINT
        {
            return self.open_target(target_key).await;
        }
        let mode = self.inner.targets.target_mode(target_key).ok_or_else(|| {
            TenantDataError::UnknownTarget {
                target_key: target_key.into(),
            }
        })?;
        let kind = self.inner.targets.target_kind(target_key).ok_or_else(|| {
            TenantDataError::UnknownTarget {
                target_key: target_key.into(),
            }
        })?;
        let database = self.database_for_target(target_key).await?;
        self.verify_database_now(&database, target_key, mode, catalog, true)
            .await?;
        Ok(TenantDataTargetHandle {
            target_key: target_key.into(),
            mode,
            kind,
            database,
        })
    }

    pub async fn verify_target_now(&self, target_key: &str) -> Result<(), TenantDataError> {
        self.verify_target_now_for_catalog(
            target_key,
            &ryframe_tenant_db_migration::TENANT_DATA_CATALOG,
        )
        .await
    }

    pub async fn verify_target_now_for_catalog(
        &self,
        target_key: &str,
        catalog: &ryframe_tenant_db_migration::TenantDataCatalog,
    ) -> Result<(), TenantDataError> {
        let mode = self.inner.targets.target_mode(target_key).ok_or_else(|| {
            TenantDataError::UnknownTarget {
                target_key: target_key.into(),
            }
        })?;
        let database = self.database_for_target(target_key).await?;
        self.verify_database_now(&database, target_key, mode, catalog, true)
            .await
    }

    pub const fn shared_control_target_key() -> &'static str {
        SHARED_CONTROL_TARGET_KEY
    }
    pub(super) async fn verified_database_for_target(
        &self,
        target_key: &str,
    ) -> Result<SessionDatabase, TenantDataError> {
        let database = self.database_for_target(target_key).await?;
        let mode = self.inner.targets.target_mode(target_key).ok_or_else(|| {
            TenantDataError::UnknownTarget {
                target_key: target_key.into(),
            }
        })?;
        let health = self.inner.targets.target_health(target_key);
        if health == Some(crate::TenantDatabaseTargetHealthStatus::Unknown) {
            let initial = match &database {
                SessionDatabase::SharedControl(control) => self
                    .inner
                    .control_schema_verification
                    .get_or_init(|| async {
                        verify_schema_for_catalog(
                            control.write(),
                            target_key,
                            &ryframe_tenant_db_migration::TENANT_DATA_CATALOG,
                        )
                        .await
                    })
                    .await
                    .clone(),
                SessionDatabase::Mysql(lease) => lease.ensure_schema_verified().await,
            };
            if let Err(error) = initial {
                self.inner.targets.mark_target_unavailable(target_key);
                return Err(error);
            }
            if let Err(error) =
                verify_target_mode_invariants(database.writer(), target_key, mode).await
            {
                self.inner.targets.mark_target_unavailable(target_key);
                return Err(error);
            }
            if matches!(database, SessionDatabase::SharedControl(_)) {
                self.inner.targets.mark_control_schema_verified();
            } else {
                self.inner.targets.mark_target_verified(target_key);
            }
        } else if self
            .inner
            .targets
            .target_health_is_stale(target_key, Duration::from_secs(60))
        {
            self.verify_database_now(
                &database,
                target_key,
                mode,
                &ryframe_tenant_db_migration::TENANT_DATA_CATALOG,
                false,
            )
            .await?;
        }
        Ok(database)
    }

    async fn verify_database_now(
        &self,
        database: &SessionDatabase,
        target_key: &str,
        mode: TenantDatabaseTargetMode,
        catalog: &ryframe_tenant_db_migration::TenantDataCatalog,
        force: bool,
    ) -> Result<(), TenantDataError> {
        let result = match database {
            SessionDatabase::SharedControl(control) => {
                let _guard = self.inner.control_fresh_verification.lock().await;
                if !force
                    && !self
                        .inner
                        .targets
                        .target_health_is_stale(target_key, Duration::from_secs(60))
                {
                    Ok(false)
                } else {
                    async {
                        ryframe_db::connection::ping(control.write())
                            .await
                            .map_err(|_| TenantDataError::TargetUnavailable {
                                target_key: target_key.into(),
                            })?;
                        verify_schema_for_catalog(control.write(), target_key, catalog)
                            .await
                            .map(|()| true)
                    }
                    .await
                }
            }
            SessionDatabase::Mysql(lease) => {
                if force {
                    lease
                        .verify_schema_now_for_catalog(catalog)
                        .await
                        .map(|()| true)
                } else {
                    lease
                        .verify_schema_if_stale_for_catalog(catalog, Duration::from_secs(60))
                        .await
                }
            }
        };
        let verified_now = result.inspect_err(|_| {
            self.inner.targets.mark_target_unavailable(target_key);
        })?;
        if !verified_now {
            return Ok(());
        }
        if let Err(error) = verify_target_mode_invariants(database.writer(), target_key, mode).await
        {
            self.inner.targets.mark_target_unavailable(target_key);
            return Err(error);
        }
        if matches!(database, SessionDatabase::SharedControl(_)) {
            self.inner.targets.mark_control_schema_verified();
        } else {
            self.inner.targets.mark_target_verified(target_key);
        }
        Ok(())
    }

    async fn database_for_target(
        &self,
        target_key: &str,
    ) -> Result<SessionDatabase, TenantDataError> {
        match self.inner.targets.target_kind(target_key) {
            Some(TenantDatabaseTargetKind::Control) => {
                Ok(SessionDatabase::SharedControl(self.inner.control.clone()))
            }
            Some(TenantDatabaseTargetKind::Mysql) => Ok(SessionDatabase::Mysql(
                self.inner.targets.acquire(target_key).await?,
            )),
            None => Err(TenantDataError::UnknownTarget {
                target_key: target_key.into(),
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum SessionDatabase {
    SharedControl(ControlDatabaseCluster),
    Mysql(TenantDatabasePoolLease),
}

impl SessionDatabase {
    pub(super) fn writer(&self) -> &DatabaseConnection {
        match self {
            Self::SharedControl(control) => control.write(),
            Self::Mysql(lease) => lease.connection(),
        }
    }
}

impl TenantDataTargetHandle {
    pub fn target_key(&self) -> &str {
        &self.target_key
    }

    pub const fn mode(&self) -> TenantDatabaseTargetMode {
        self.mode
    }

    pub const fn kind(&self) -> TenantDatabaseTargetKind {
        self.kind
    }

    pub fn connection(&self) -> &DatabaseConnection {
        self.database.writer()
    }

    pub fn schema_fingerprint(&self) -> &'static str {
        ryframe_tenant_db_migration::TENANT_DATA_SCHEMA_FINGERPRINT
    }
}

async fn verify_schema_for_catalog(
    database: &DatabaseConnection,
    target_key: &str,
    catalog: &ryframe_tenant_db_migration::TenantDataCatalog,
) -> Result<(), TenantDataError> {
    ryframe_tenant_db_migration::verify_for_catalog(database, catalog)
        .await
        .map_err(|error| {
            tracing::warn!(
                target = target_key,
                %error,
                "租户数据目标 migration ledger 或 schema 指纹不兼容"
            );
            TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            }
        })
}

async fn verify_target_mode_invariants(
    database: &DatabaseConnection,
    target_key: &str,
    target_mode: TenantDatabaseTargetMode,
) -> Result<(), TenantDataError> {
    let slot = TargetSlotRow::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        TARGET_SLOT_QUERY,
    ))
    .one(database)
    .await
    .map_err(|error| {
        tracing::warn!(target = target_key, %error, "目标模式占用槽校验失败");
        TenantDataError::TargetUnavailable {
            target_key: target_key.into(),
        }
    })?
    .ok_or_else(|| TenantDataError::TargetUnavailable {
        target_key: target_key.into(),
    })?;
    let slot_empty = matches!(
        (
            &slot.tenant_id,
            slot.placement_generation,
            &slot.switch_token
        ),
        (None, None, None)
    );
    let valid = match target_mode {
        TenantDatabaseTargetMode::Shared => {
            if !slot_empty {
                false
            } else {
                scalar_exists(
                    database,
                    "SELECT EXISTS(SELECT 1 FROM biz_tenant_fence \
                     WHERE target_key <> ? OR placement_generation <= 0 OR switch_token = '' \
                        OR state NOT IN ('active', 'frozen') LIMIT 1)",
                    [target_key.into()],
                    target_key,
                )
                .await
                .map(|invalid| !invalid)?
            }
        }
        TenantDatabaseTargetMode::Dedicated => {
            if slot_empty {
                !scalar_exists(
                    database,
                    "SELECT EXISTS(SELECT 1 FROM biz_tenant_fence LIMIT 1)",
                    [],
                    target_key,
                )
                .await?
            } else if let (Some(tenant_id), Some(generation), Some(switch_token)) = (
                slot.tenant_id.as_deref(),
                slot.placement_generation,
                slot.switch_token.as_deref(),
            ) {
                if generation <= 0 || switch_token.is_empty() {
                    false
                } else {
                    let row = database
                        .query_one_raw(Statement::from_sql_and_values(
                            DbBackend::MySql,
                            "SELECT CAST(COUNT(*) AS SIGNED) AS total_count, \
                             CAST(COALESCE(SUM(tenant_id = ? \
                             AND target_key = ? AND placement_generation = ? \
                             AND switch_token = ? AND state IN ('active', 'frozen')), 0) \
                             AS SIGNED) AS exact_count FROM biz_tenant_fence",
                            [
                                tenant_id.into(),
                                target_key.into(),
                                generation.into(),
                                switch_token.into(),
                            ],
                        ))
                        .await
                        .map_err(|error| {
                            tracing::warn!(target = target_key, %error, "dedicated fence 聚合校验失败");
                            TenantDataError::TargetUnavailable {
                                target_key: target_key.into(),
                            }
                        })?
                        .ok_or_else(|| TenantDataError::TargetUnavailable {
                            target_key: target_key.into(),
                        })?;
                    row.try_get_by_index::<i64>(0).ok() == Some(1)
                        && row.try_get_by_index::<i64>(1).ok() == Some(1)
                }
            } else {
                false
            }
        }
    };
    if !valid {
        tracing::warn!(target = target_key, mode = ?target_mode, "租户数据目标 mode/fence/slot 不变量不一致");
        return Err(TenantDataError::TargetUnavailable {
            target_key: target_key.into(),
        });
    }
    Ok(())
}

async fn scalar_exists<I>(
    database: &DatabaseConnection,
    sql: &str,
    values: I,
    target_key: &str,
) -> Result<bool, TenantDataError>
where
    I: IntoIterator<Item = sea_orm::Value>,
{
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            sql,
            values,
        ))
        .await
        .map_err(|error| {
            tracing::warn!(target = target_key, %error, "目标不变量有界查询失败");
            TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            }
        })?
        .ok_or_else(|| TenantDataError::TargetUnavailable {
            target_key: target_key.into(),
        })?;
    row.try_get_by_index::<i64>(0)
        .map(|value| value != 0)
        .map_err(|error| {
            tracing::warn!(target = target_key, %error, "目标不变量结果无效");
            TenantDataError::TargetUnavailable {
                target_key: target_key.into(),
            }
        })
}
