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
pub struct ProductPlanState {
    pub id: i64,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub created_by: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct ProductVersionState {
    pub id: i64,
    pub plan_id: i64,
    pub version: i32,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub created_by: i64,
    pub published_by: Option<i64>,
    pub published_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct ProductVersionWriteResult {
    pub version: ProductVersionState,
    pub capabilities: Vec<ProductCapabilityRecord>,
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

#[derive(Clone, Debug)]
pub struct ProvisioningCapabilityResources {
    pub enabled_route_keys: Vec<String>,
    pub enabled_permission_codes: Vec<String>,
    pub managed_route_keys: Vec<String>,
    pub managed_permission_codes: Vec<String>,
    pub default_admin_permissions: Vec<String>,
}

#[derive(Debug)]
pub struct ProductChangeTenantState {
    pub status: String,
    pub authorization_epoch: i32,
    pub runtime_epoch: i64,
    pub database_now: DateTime<Utc>,
}

#[derive(Debug)]
pub struct ProductAssignmentChange {
    pub tenant_id: String,
    pub version_id: i64,
    pub changed_by: i64,
    pub reason: Option<String>,
    pub overrides: Vec<TenantCapabilityOverrideRecord>,
    pub changed_at: DateTime<Utc>,
}

pub trait ProductReadPort: Send + Sync {
    fn list_plans(&self) -> PersistenceFuture<'_, Vec<ProductPlanRecord>>;

    fn find_plan(&self, plan_id: i64) -> PersistenceFuture<'_, Option<ProductPlanRecord>>;

    fn find_version(
        &self,
        version_id: i64,
    ) -> PersistenceFuture<'_, Option<ProductVersionSnapshot>>;

    fn tenant_product<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantProductSnapshot>>;
}

pub trait ProductWriteTransaction: Send + Sync {
    fn lock_change_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, ProductChangeTenantState>;

    fn acquire_change_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
        version_id: i64,
        acquired_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> PersistenceFuture<'a, ()>;

    fn lock_assignable_version(
        &self,
        version_id: i64,
    ) -> PersistenceFuture<'_, ProductVersionSnapshot>;

    fn current_tenant_product<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, TenantProductSnapshot>;

    fn sync_capability_resources<'a>(
        &'a self,
        tenant_id: &'a str,
        resources: &'a ProvisioningCapabilityResources,
    ) -> PersistenceFuture<'a, ()>;

    fn replace_assignment(&self, change: ProductAssignmentChange) -> PersistenceFuture<'_, ()>;

    fn increment_runtime_epoch<'a>(
        &'a self,
        tenant_id: &'a str,
        expected_epoch: i64,
    ) -> PersistenceFuture<'a, ()>;

    fn authorization_mirror(&self) -> &dyn crate::AuthorizationMirrorTransaction;

    fn release_change_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
    ) -> PersistenceFuture<'a, ()>;

    fn plan_key_exists<'a>(&'a self, key: &'a str) -> PersistenceFuture<'a, bool>;

    fn insert_plan(&self, plan: ProductPlanState) -> PersistenceFuture<'_, ProductPlanState>;

    fn lock_plan(&self, plan_id: i64) -> PersistenceFuture<'_, ProductPlanState>;

    fn save_plan(&self, plan: ProductPlanState) -> PersistenceFuture<'_, ProductPlanState>;

    fn next_version(&self, plan_id: i64) -> PersistenceFuture<'_, i32>;

    fn insert_version(
        &self,
        version: ProductVersionState,
        capabilities: Vec<ProductCapabilityRecord>,
        capability_time: DateTime<Utc>,
    ) -> PersistenceFuture<'_, ProductVersionWriteResult>;

    fn lock_version(
        &self,
        plan_id: i64,
        version: i32,
    ) -> PersistenceFuture<'_, ProductVersionState>;

    fn capabilities(&self, version_id: i64) -> PersistenceFuture<'_, Vec<ProductCapabilityRecord>>;

    fn replace_draft_version(
        &self,
        version: ProductVersionState,
        capabilities: Vec<ProductCapabilityRecord>,
        capability_time: DateTime<Utc>,
    ) -> PersistenceFuture<'_, ProductVersionWriteResult>;

    fn transition_version(
        &self,
        version: ProductVersionState,
        expected_status: &str,
        target_status: &str,
    ) -> PersistenceFuture<'_, ProductVersionState>;

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

pub trait ProductWritePort: Send + Sync {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ProductWriteTransaction>>;
}
