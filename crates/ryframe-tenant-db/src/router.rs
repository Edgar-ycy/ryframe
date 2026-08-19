use std::{sync::Arc, time::Duration};

use ryframe_config::{
    SHARED_CONTROL_TARGET_KEY, TenantDataConfig, TenantDatabaseTargetKind, TenantDatabaseTargetMode,
};
use ryframe_db::{ControlDatabaseCluster, DatabaseNodeKind, ReadConsistency, SelectedDatabase};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, FromQueryResult,
    Statement, TransactionTrait,
};
use tokio::sync::OnceCell;

use crate::{
    PendingTenantDataPlacement, TenantDataAccess, TenantDataError, TenantDataPlacement,
    TenantDataState, TenantDatabasePoolLease, TenantDatabaseTargetRegistry, TenantRuntimeSnapshot,
};

const PLACEMENT_QUERY: &str = "SELECT tenant_id, current_target_key, placement_generation, state, switch_token \
FROM sys_tenant_data_placement WHERE tenant_id = ? LIMIT 1";
const RUNTIME_SNAPSHOT_QUERY: &str = "SELECT tenant.tenant_id, tenant.authorization_epoch, tenant.runtime_epoch, \
placement.placement_generation, placement.state AS business_data_state \
FROM sys_tenant AS tenant INNER JOIN sys_tenant_data_placement AS placement \
ON placement.tenant_id = tenant.tenant_id WHERE tenant.tenant_id = ? LIMIT 1";
const CURRENT_TARGETS_QUERY: &str = "SELECT current_target_key, CAST(COUNT(*) AS SIGNED) AS tenant_count \
FROM sys_tenant_data_placement WHERE state = 'active' \
GROUP BY current_target_key ORDER BY current_target_key";
const PLACEMENT_METRICS_QUERY: &str = "SELECT current_target_key, state, CAST(COUNT(*) AS SIGNED) AS tenant_count \
FROM sys_tenant_data_placement GROUP BY current_target_key, state \
ORDER BY current_target_key, state";
const FENCE_QUERY: &str = "SELECT tenant_id, target_key, placement_generation, state, switch_token \
FROM biz_tenant_fence WHERE tenant_id = ? LIMIT 1";
const FENCE_LOCK_QUERY: &str = "SELECT tenant_id, target_key, placement_generation, state, switch_token \
FROM biz_tenant_fence WHERE tenant_id = ? LIMIT 1 FOR UPDATE";
const TARGET_SLOT_QUERY: &str = "SELECT tenant_id, placement_generation, switch_token \
FROM biz_tenant_target_slot WHERE slot_id = 1 LIMIT 1";
const TARGET_SLOT_LOCK_QUERY: &str = "SELECT tenant_id, placement_generation, switch_token \
FROM biz_tenant_target_slot WHERE slot_id = 1 LIMIT 1 FOR UPDATE";

#[derive(Debug, FromQueryResult)]
struct PlacementRow {
    tenant_id: String,
    current_target_key: String,
    placement_generation: i64,
    state: String,
    switch_token: String,
}

#[derive(Debug, FromQueryResult)]
struct RuntimeSnapshotRow {
    tenant_id: String,
    authorization_epoch: i64,
    runtime_epoch: i64,
    placement_generation: i64,
    business_data_state: String,
}

#[derive(Debug, FromQueryResult)]
struct CurrentTargetRow {
    current_target_key: String,
    tenant_count: i64,
}

#[derive(Debug, FromQueryResult)]
struct PlacementMetricRow {
    current_target_key: String,
    state: String,
    tenant_count: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TenantDataTargetHealth {
    Verified,
    UnknownTarget,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantDataTargetVerification {
    pub target_key: String,
    pub tenant_count: u64,
    pub mode: Option<TenantDatabaseTargetMode>,
    pub health: TenantDataTargetHealth,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantDataPlacementMetric {
    pub mode: Option<TenantDatabaseTargetMode>,
    pub state: TenantDataState,
    pub tenant_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantDataTargetOccupancy {
    pub tenant_id: String,
    pub placement_generation: i64,
    pub switch_token: String,
}

#[derive(Debug, FromQueryResult)]
struct FenceRow {
    tenant_id: String,
    target_key: String,
    placement_generation: i64,
    state: String,
    switch_token: String,
}

#[derive(Debug, FromQueryResult)]
struct TargetSlotRow {
    tenant_id: Option<String>,
    placement_generation: Option<i64>,
    switch_token: Option<String>,
}

#[derive(Clone, Debug)]
struct FenceProvision {
    tenant_id: String,
    target_key: String,
    placement_generation: i64,
    switch_token: String,
}

#[derive(Debug)]
struct RouterInner {
    control: ControlDatabaseCluster,
    targets: TenantDatabaseTargetRegistry,
    control_schema_verification: OnceCell<Result<(), TenantDataError>>,
    control_fresh_verification: tokio::sync::Mutex<()>,
}

/// 从控制库权威 placement 解析租户数据 Session 的路由器。
///
/// 每个用例都从控制库 writer 强一致读取 placement，不把缓存失效事件当成安全边界。
/// 随后在所选目标校验租户数据 migration ledger、schema 指纹和 fence。
#[derive(Clone, Debug)]
pub struct TenantDatabaseRouter {
    inner: Arc<RouterInner>,
}

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

    pub async fn target_occupancy(
        &self,
        target_key: &str,
    ) -> Result<Option<TenantDataTargetOccupancy>, TenantDataError> {
        self.target_occupancy_for_catalog(
            target_key,
            &ryframe_tenant_db_migration::TENANT_DATA_CATALOG,
        )
        .await
    }

    pub async fn target_occupancy_for_catalog(
        &self,
        target_key: &str,
        catalog: &ryframe_tenant_db_migration::TenantDataCatalog,
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
            &ryframe_tenant_db_migration::TENANT_DATA_CATALOG,
        )
        .await
    }

    pub async fn tenant_is_empty_on_target_for_catalog(
        &self,
        target_key: &str,
        tenant_id: &str,
        catalog: &ryframe_tenant_db_migration::TenantDataCatalog,
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
            &ryframe_tenant_db_migration::TENANT_DATA_CATALOG,
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
        catalog: &ryframe_tenant_db_migration::TenantDataCatalog,
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
        catalog: &ryframe_tenant_db_migration::TenantDataCatalog,
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
        catalog: &ryframe_tenant_db_migration::TenantDataCatalog,
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

    pub const fn shared_control_target_key() -> &'static str {
        SHARED_CONTROL_TARGET_KEY
    }

    /// 只访问控制库 writer 的单条强一致查询，不打开或校验任何租户数据目标。
    pub async fn runtime_snapshot(
        &self,
        tenant_id: &str,
    ) -> Result<TenantRuntimeSnapshot, TenantDataError> {
        ryframe_core::validate_tenant_identifier(tenant_id)
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

    /// 启动时校验当前 placement 实际引用的目标；单个故障只进入报告，不拖垮平台控制面。
    pub async fn verify_current_targets(
        &self,
    ) -> Result<Vec<TenantDataTargetVerification>, TenantDataError> {
        let rows = CurrentTargetRow::find_by_statement(Statement::from_string(
            DbBackend::MySql,
            CURRENT_TARGETS_QUERY,
        ))
        .all(self.inner.control.write())
        .await
        .map_err(|error| {
            tracing::warn!(%error, "当前租户数据目标集合查询失败");
            TenantDataError::InvalidConfiguration("sys_tenant_data_placement 无法强一致读取".into())
        })?;

        let concurrency = Arc::new(tokio::sync::Semaphore::new(8));
        let mut tasks = tokio::task::JoinSet::new();
        for row in rows {
            let router = self.clone();
            let concurrency = concurrency.clone();
            tasks.spawn(async move {
                let tenant_count = row.tenant_count.max(0) as u64;
                let mode = router.inner.targets.target_mode(&row.current_target_key);
                let health = if mode.is_none() {
                    TenantDataTargetHealth::UnknownTarget
                } else {
                    let _permit = concurrency.acquire_owned().await.ok();
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        router.verify_target_now(&row.current_target_key),
                    )
                    .await
                    {
                        Ok(Ok(_)) => TenantDataTargetHealth::Verified,
                        Ok(Err(error)) => {
                            tracing::warn!(
                                target = %row.current_target_key,
                                tenant_count,
                                %error,
                                "租户数据目标连接失败"
                            );
                            TenantDataTargetHealth::Unavailable
                        }
                        Err(_) => {
                            tracing::warn!(
                                target = %row.current_target_key,
                                tenant_count,
                                "租户数据目标启动校验超时"
                            );
                            TenantDataTargetHealth::Unavailable
                        }
                    }
                };
                TenantDataTargetVerification {
                    target_key: row.current_target_key,
                    tenant_count,
                    mode,
                    health,
                }
            });
        }
        let mut reports = Vec::with_capacity(tasks.len());
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(report) => reports.push(report),
                Err(error) => tracing::error!(%error, "租户数据目标启动校验任务异常退出"),
            }
        }
        reports.sort_unstable_by(|left, right| left.target_key.cmp(&right.target_key));
        Ok(reports)
    }

    /// 只读控制库的低基数 placement 聚合；不连接任何目标且不包含 tenant/target 标签。
    pub async fn placement_metrics_snapshot(
        &self,
    ) -> Result<Vec<TenantDataPlacementMetric>, TenantDataError> {
        let rows = PlacementMetricRow::find_by_statement(Statement::from_string(
            DbBackend::MySql,
            PLACEMENT_METRICS_QUERY,
        ))
        .all(self.inner.control.write())
        .await
        .map_err(|error| {
            tracing::warn!(%error, "租户数据 placement 指标聚合失败");
            TenantDataError::InvalidConfiguration("sys_tenant_data_placement 指标无法读取".into())
        })?;
        rows.into_iter()
            .map(|row| {
                let state = TenantDataState::parse(&row.state).ok_or_else(|| {
                    TenantDataError::InvalidConfiguration(
                        "sys_tenant_data_placement 包含未知状态".into(),
                    )
                })?;
                Ok(TenantDataPlacementMetric {
                    mode: self.inner.targets.target_mode(&row.current_target_key),
                    state,
                    tenant_count: row.tenant_count.max(0) as u64,
                })
            })
            .collect()
    }

    /// 使用注册表中的批准目标构造租户创建 Saga 输入，不接受连接信息或任意 DSN。
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

    fn prepare_fence_provision(
        &self,
        tenant_id: &str,
        target_key: &str,
        placement_generation: i64,
        switch_token: &str,
    ) -> Result<FenceProvision, TenantDataError> {
        ryframe_core::validate_tenant_identifier(tenant_id)
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

    /// 强一致解析 placement，并在返回前校验目标 schema 和 fence。
    pub async fn resolve(&self, tenant_id: &str) -> Result<TenantDataSession, TenantDataError> {
        ryframe_core::validate_tenant_identifier(tenant_id)
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

    async fn verified_database_for_target(
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

#[derive(Clone, Debug)]
enum SessionDatabase {
    SharedControl(ControlDatabaseCluster),
    Mysql(TenantDatabasePoolLease),
}

impl SessionDatabase {
    fn writer(&self) -> &DatabaseConnection {
        match self {
            Self::SharedControl(control) => control.write(),
            Self::Mysql(lease) => lease.connection(),
        }
    }
}

/// 平台迁移服务持有的已批准、已校验目标句柄；不会暴露配置凭据或 DSN。
#[derive(Clone, Debug)]
pub struct TenantDataTargetHandle {
    target_key: String,
    mode: TenantDatabaseTargetMode,
    kind: TenantDatabaseTargetKind,
    database: SessionDatabase,
}

/// 一次 migration-owned catalog 表分批清理请求。
#[derive(Clone, Copy, Debug)]
pub struct TenantDataCleanupBatch<'a> {
    pub tenant_id: &'a str,
    pub target_key: &'a str,
    pub placement_generation: i64,
    pub switch_token: &'a str,
    pub descriptor: &'a ryframe_tenant_db_migration::TenantDataTableDescriptor,
    pub batch_size: u32,
}

/// migration cleanup 对目标数据/fence 的权威所有权判定。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TenantDataCleanupOwnership {
    /// exact generation/token 的 frozen fence（dedicated 时槽也 exact）。
    OwnedFrozen,
    /// fence/slot 已清理且 catalog 已空，幂等完成。
    AlreadyClean,
    /// 存在不属于本 migration token 的 fence/slot/catalog 数据，绝不允许删除。
    NotOwned,
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

/// 固定租户、目标、数据状态和 placement generation 的数据 Session。
#[derive(Clone, Debug)]
pub struct TenantDataSession {
    placement: TenantDataPlacement,
    target_mode: TenantDatabaseTargetMode,
    database: SessionDatabase,
}

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

    async fn verify_target_fence(&self, access: TenantDataAccess) -> Result<(), TenantDataError> {
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

async fn prepare_frozen_fence_in_transaction(
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
    catalog: &ryframe_tenant_db_migration::TenantDataCatalog,
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

async fn set_fence_state_in_transaction(
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

fn require_owned_frozen_fence(
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

fn require_exact_dedicated_slot(
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

async fn lock_target_slot(
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

async fn lock_fence(
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

fn fence_matches(fence: &FenceRow, provision: &FenceProvision) -> bool {
    fence.tenant_id == provision.tenant_id
        && fence.target_key == provision.target_key
        && fence.placement_generation > 0
        && fence.placement_generation == provision.placement_generation
        && fence.switch_token == provision.switch_token
}

fn target_write_error(
    provision: &FenceProvision,
    error: impl std::fmt::Display,
    operation: &'static str,
) -> TenantDataError {
    tracing::warn!(tenant_id = %provision.tenant_id, target = %provision.target_key, %error, operation);
    TenantDataError::TargetUnavailable {
        target_key: provision.target_key.clone(),
    }
}

async fn finish_target_transaction(
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

async fn claim_dedicated_slot(
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
