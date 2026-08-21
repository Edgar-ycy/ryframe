use std::{future::Future, pin::Pin};

use chrono::{DateTime, Utc};
use ryframe_kernel::AppResult;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TenantDataTargetHealth {
    Unknown,
    Verified,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantDataTargetMetadata {
    pub key: String,
    pub display_name: Option<String>,
    pub region: Option<String>,
    pub mode: String,
    pub kind: String,
    pub connected: bool,
    pub pool_max_connections: Option<u32>,
    pub active_leases: usize,
    pub schema_fingerprint: Option<String>,
    pub health: TenantDataTargetHealth,
    pub last_verified_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TenantDataTargetAccess {
    pub dedicated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantDataPoolStats {
    pub reserved_connections: u32,
    pub max_total_connections: u32,
    pub open_targets: usize,
    pub opening_targets: usize,
}

pub type TenantDataTargetFuture<'a, T> = Pin<Box<dyn Future<Output = AppResult<T>> + Send + 'a>>;

/// 租户数据目标目录、健康状态与迁移前置检查端口。
pub trait TenantDataTargetPort: Send + Sync {
    fn contains(&self, target_key: &str) -> bool;
    fn is_dedicated(&self, target_key: &str) -> Option<bool>;
    fn mode_code(&self, target_key: &str) -> Option<&'static str>;
    fn kind_code(&self, target_key: &str) -> Option<&'static str>;
    fn catalog_fingerprint(&self) -> String;
    fn catalog_table_count(&self) -> usize;

    fn metadata(&self) -> TenantDataTargetFuture<'_, Vec<TenantDataTargetMetadata>>;
    fn pool_stats(&self) -> TenantDataTargetFuture<'_, TenantDataPoolStats>;
    fn verify_now<'a>(&'a self, target_key: &'a str) -> TenantDataTargetFuture<'a, ()>;
    fn validate_catalog<'a>(
        &'a self,
        target_key: &'a str,
    ) -> TenantDataTargetFuture<'a, TenantDataTargetAccess>;
    fn is_occupied<'a>(&'a self, target_key: &'a str) -> TenantDataTargetFuture<'a, bool>;
    fn tenant_is_empty<'a>(
        &'a self,
        target_key: &'a str,
        tenant_id: &'a str,
    ) -> TenantDataTargetFuture<'a, bool>;
}
