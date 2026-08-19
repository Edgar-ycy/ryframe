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

/// 平台迁移服务持有的已批准、已校验目标句柄；不会暴露配置凭据或 DSN。
#[derive(Clone, Debug)]
pub struct TenantDataTargetHandle {
    target_key: String,
    mode: TenantDatabaseTargetMode,
    kind: TenantDatabaseTargetKind,
    database: connection::SessionDatabase,
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

/// 固定租户、目标、数据状态和 placement generation 的数据 Session。
#[derive(Clone, Debug)]
pub struct TenantDataSession {
    placement: TenantDataPlacement,
    target_mode: TenantDatabaseTargetMode,
    database: connection::SessionDatabase,
}

mod cleanup;
mod connection;
mod fence;
mod metrics;
mod migration;
mod placement;
mod session;
