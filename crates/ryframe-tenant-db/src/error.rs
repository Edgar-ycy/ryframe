use thiserror::Error;

/// 租户数据面路由的稳定错误分类。
///
/// HTTP、Worker 和 CLI 应按枚举分支适配，不得依赖展示消息判断失败原因。
#[derive(Clone, Debug, Error)]
pub enum TenantDataError {
    #[error("租户数据面配置无效: {0}")]
    InvalidConfiguration(String),
    #[error("租户标识无效: {0}")]
    InvalidTenantId(String),
    #[error("租户数据目标不存在: {target_key}")]
    UnknownTarget { target_key: String },
    #[error("租户 {tenant_id} 没有可用的权威数据 placement")]
    PlacementUnavailable { tenant_id: String },
    #[error("租户 {tenant_id} 的数据 placement 无效: {reason}")]
    InvalidPlacement { tenant_id: String, reason: String },
    #[error(
        "租户 {tenant_id} 的 placement generation 已过期: session={session_generation}, current={current_generation}"
    )]
    StalePlacementGeneration {
        tenant_id: String,
        session_generation: i64,
        current_generation: i64,
    },
    #[error("租户 {tenant_id} 的数据 placement 正在维护，generation={generation}")]
    TenantDataMaintenance { tenant_id: String, generation: i64 },
    #[error("租户数据目标 {target_key} 暂不可用")]
    TargetUnavailable { target_key: String },
    #[error("租户数据连接池预算已耗尽: open={open_targets}, limit={max_open_targets}")]
    PoolCapacityExhausted {
        open_targets: usize,
        max_open_targets: usize,
    },
    #[error("租户数据连接预算已耗尽: used={used}, requested={requested}, limit={limit}")]
    ConnectionBudgetExhausted {
        used: u32,
        requested: u32,
        limit: u32,
    },
    #[error("租户 {tenant_id} 在目标 {target_key} 上没有有效 fence: {reason}")]
    FenceRejected {
        tenant_id: String,
        target_key: String,
        reason: String,
    },
    #[error("dedicated 目标 {target_key} 已存在其他 active 租户")]
    DedicatedTargetOccupied { target_key: String },
}
