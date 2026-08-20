use chrono::{DateTime, Utc};

use crate::PersistenceFuture;

#[derive(Debug)]
pub struct ProductCapabilityRecord {
    pub code: String,
    pub variant: String,
    pub schema_version: i32,
    pub config: serde_json::Value,
}

#[derive(Debug)]
pub struct ProductVersionRecord {
    pub id: i64,
    pub version: i32,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub created_by: i64,
    pub published_by: Option<i64>,
    pub published_at: Option<DateTime<Utc>>,
    pub capabilities: Vec<ProductCapabilityRecord>,
}

#[derive(Debug)]
pub struct ProductPlanRecord {
    pub id: i64,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub created_by: i64,
    pub versions: Vec<ProductVersionRecord>,
}

#[derive(Debug)]
pub struct ProductVersionSnapshot {
    pub plan_key: String,
    pub plan_name: String,
    pub plan_status: String,
    pub version_id: i64,
    pub version: i32,
    pub version_status: String,
    pub capabilities: Vec<ProductCapabilityRecord>,
}

#[derive(Debug)]
pub struct TenantCapabilityOverrideRecord {
    pub code: String,
    pub enabled: bool,
    pub variant: String,
    pub schema_version: i32,
    pub config: serde_json::Value,
    pub reason: Option<String>,
    pub changed_by: Option<i64>,
}

#[derive(Debug)]
pub struct TenantProductSnapshot {
    pub tenant_id: String,
    pub authorization_epoch: i32,
    pub runtime_epoch: i64,
    pub version: ProductVersionSnapshot,
    pub overrides: Vec<TenantCapabilityOverrideRecord>,
}

pub trait ProductReadPort: Send + Sync {
    fn list_plans(&self) -> PersistenceFuture<'_, Vec<ProductPlanRecord>>;

    fn find_plan(&self, plan_id: i64) -> PersistenceFuture<'_, Option<ProductPlanRecord>>;

    fn tenant_product<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantProductSnapshot>>;
}
