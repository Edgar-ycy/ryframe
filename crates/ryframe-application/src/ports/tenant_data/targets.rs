use async_trait::async_trait;
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

/// 租户数据目标目录、健康状态与迁移前置检查端口。
#[async_trait]
pub trait TenantDataTargetPort: Send + Sync {
    fn contains(&self, target_key: &str) -> bool;
    fn is_dedicated(&self, target_key: &str) -> Option<bool>;
    fn mode_code(&self, target_key: &str) -> Option<&'static str>;
    fn kind_code(&self, target_key: &str) -> Option<&'static str>;
    fn catalog_fingerprint(&self) -> String;
    fn catalog_table_count(&self) -> usize;

    async fn metadata(&self) -> AppResult<Vec<TenantDataTargetMetadata>>;
    async fn pool_stats(&self) -> AppResult<TenantDataPoolStats>;
    async fn verify_now(&self, target_key: &str) -> AppResult<()>;
    async fn validate_catalog(&self, target_key: &str) -> AppResult<TenantDataTargetAccess>;
    async fn is_occupied(&self, target_key: &str) -> AppResult<bool>;
    async fn tenant_is_empty(&self, target_key: &str, tenant_id: &str) -> AppResult<bool>;
}
