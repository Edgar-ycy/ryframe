//! 租户数据目标注册、延迟连接和 placement fence。
//!
//! 该 crate 只提供基础设施路由，不拥有控制面 placement API，也不执行租户迁移状态机。
//! `database.sources` 仍由控制库集群显式管理，不进入本路由器。

#[cfg(feature = "repositories")]
pub mod application_ports;
#[cfg(feature = "connection")]
mod error;
#[cfg(feature = "connection")]
pub mod generated;
#[cfg(feature = "connection")]
pub mod migration;
#[cfg(feature = "repositories")]
mod placement;
#[cfg(feature = "repositories")]
mod placement_repo;
#[cfg(feature = "connection")]
mod registry;
#[cfg(feature = "repositories")]
mod router;

#[cfg(feature = "connection")]
pub use error::TenantDataError;
#[cfg(feature = "repositories")]
pub use placement::{
    TenantDataAccess, TenantDataPlacement, TenantDataState, TenantRuntimeSnapshot,
};
#[cfg(feature = "repositories")]
pub use placement_repo::{PendingTenantDataPlacement, TenantDataPlacementRepository};
#[cfg(feature = "connection")]
pub use registry::{
    TenantDatabasePoolLease, TenantDatabasePoolStats, TenantDatabaseTargetHealthStatus,
    TenantDatabaseTargetMetadata, TenantDatabaseTargetRegistry,
};
#[cfg(feature = "repositories")]
pub use router::{
    TenantDataCleanupBatch, TenantDataCleanupOwnership, TenantDataPlacementMetric,
    TenantDataSession, TenantDataTargetHandle, TenantDataTargetHealth, TenantDataTargetOccupancy,
    TenantDataTargetVerification, TenantDatabaseRouter,
};
#[cfg(feature = "connection")]
pub use ryframe_config::SHARED_CONTROL_TARGET_KEY;
#[cfg(feature = "connection")]
pub use ryframe_db::DbResultExt;
