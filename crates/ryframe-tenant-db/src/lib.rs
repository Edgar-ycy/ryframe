//! 租户数据目标注册、延迟连接和 placement fence。
//!
//! 该 crate 只提供基础设施路由，不拥有控制面 placement API，也不执行租户迁移状态机。
//! `database.sources` 仍由控制库集群显式管理，不进入本路由器。

mod error;
mod placement;
mod placement_repo;
mod registry;
mod router;

pub use error::TenantDataError;
pub use placement::{
    TenantDataAccess, TenantDataPlacement, TenantDataState, TenantRuntimeSnapshot,
};
pub use placement_repo::{PendingTenantDataPlacement, TenantDataPlacementRepository};
pub use registry::{
    TenantDatabasePoolLease, TenantDatabasePoolStats, TenantDatabaseTargetHealthStatus,
    TenantDatabaseTargetMetadata, TenantDatabaseTargetRegistry,
};
pub use router::{
    TenantDataCleanupBatch, TenantDataCleanupOwnership, TenantDataPlacementMetric,
    TenantDataSession, TenantDataTargetHandle, TenantDataTargetHealth, TenantDataTargetOccupancy,
    TenantDataTargetVerification, TenantDatabaseRouter,
};
pub use ryframe_config::SHARED_CONTROL_TARGET_KEY;
