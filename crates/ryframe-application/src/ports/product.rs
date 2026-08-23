use chrono::{DateTime, Utc};

use crate::ports::authorization::AuthorizationMirrorTransaction;

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

#[async_trait::async_trait]
pub trait ProductReadPort: Send + Sync {
    async fn list_plans(&self) -> ryframe_kernel::AppResult<Vec<ProductPlanRecord>>;

    async fn find_plan(&self, plan_id: i64)
    -> ryframe_kernel::AppResult<Option<ProductPlanRecord>>;

    async fn find_version(
        &self,
        version_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ProductVersionSnapshot>>;

    async fn tenant_product<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<Option<TenantProductSnapshot>>;
}

/// 由调用方已有控制库事务提供的产品快照能力。
#[async_trait::async_trait]
pub trait ProductTransactionPort: Send + Sync {
    async fn current_tenant_product<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<TenantProductSnapshot>;

    async fn lock_assignable_version(
        &self,
        version_id: i64,
    ) -> ryframe_kernel::AppResult<ProductVersionSnapshot>;

    async fn sync_capability_resources<'a>(
        &'a self,
        tenant_id: &'a str,
        resources: &'a ProvisioningCapabilityResources,
    ) -> ryframe_kernel::AppResult<()>;
}

#[async_trait::async_trait]
pub trait ProductWriteTransaction: Send + Sync {
    async fn lock_change_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<ProductChangeTenantState>;

    async fn acquire_change_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
        version_id: i64,
        acquired_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<()>;

    async fn lock_assignable_version(
        &self,
        version_id: i64,
    ) -> ryframe_kernel::AppResult<ProductVersionSnapshot>;

    async fn current_tenant_product<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<TenantProductSnapshot>;

    async fn sync_capability_resources<'a>(
        &'a self,
        tenant_id: &'a str,
        resources: &'a ProvisioningCapabilityResources,
    ) -> ryframe_kernel::AppResult<()>;

    async fn replace_assignment(
        &self,
        change: ProductAssignmentChange,
    ) -> ryframe_kernel::AppResult<()>;

    async fn increment_runtime_epoch<'a>(
        &'a self,
        tenant_id: &'a str,
        expected_epoch: i64,
    ) -> ryframe_kernel::AppResult<()>;

    fn authorization_mirror(&self) -> &dyn AuthorizationMirrorTransaction;

    async fn release_change_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
    ) -> ryframe_kernel::AppResult<()>;

    async fn plan_key_exists<'a>(&'a self, key: &'a str) -> ryframe_kernel::AppResult<bool>;

    async fn insert_plan(
        &self,
        plan: ProductPlanState,
    ) -> ryframe_kernel::AppResult<ProductPlanState>;

    async fn lock_plan(&self, plan_id: i64) -> ryframe_kernel::AppResult<ProductPlanState>;

    async fn save_plan(
        &self,
        plan: ProductPlanState,
    ) -> ryframe_kernel::AppResult<ProductPlanState>;

    async fn next_version(&self, plan_id: i64) -> ryframe_kernel::AppResult<i32>;

    async fn insert_version(
        &self,
        version: ProductVersionState,
        capabilities: Vec<ProductCapabilityRecord>,
        capability_time: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<ProductVersionWriteResult>;

    async fn lock_version(
        &self,
        plan_id: i64,
        version: i32,
    ) -> ryframe_kernel::AppResult<ProductVersionState>;

    async fn capabilities(
        &self,
        version_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<ProductCapabilityRecord>>;

    async fn replace_draft_version(
        &self,
        version: ProductVersionState,
        capabilities: Vec<ProductCapabilityRecord>,
        capability_time: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<ProductVersionWriteResult>;

    async fn transition_version(
        &self,
        version: ProductVersionState,
        expected_status: &str,
        target_status: &str,
    ) -> ryframe_kernel::AppResult<ProductVersionState>;

    async fn commit(self: Box<Self>) -> ryframe_kernel::AppResult<()>;

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()>;
}

#[async_trait::async_trait]
pub trait ProductWritePort: Send + Sync {
    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn ProductWriteTransaction>>;
}
