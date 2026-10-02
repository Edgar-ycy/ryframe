//! 业务资源的 SeaORM 持久化实现。

pub mod generated;

pub use ryframe_db::{ControlDatabaseCluster, DbResultExt, ReadConsistency, pagination};
pub use ryframe_tenant_db::{TenantDataError, TenantDatabaseRouter};
