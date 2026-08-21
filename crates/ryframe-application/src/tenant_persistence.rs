use chrono::{DateTime, Utc};

use crate::{
    PersistenceFuture, TenantProvisioningPlacement,
    ports::authorization::AuthorizationMirrorTransaction, ports::product::ProductTransactionPort,
};

pub const TENANT_STATUS_PROVISIONING: &str = "provisioning";
pub const TENANT_STATUS_ENABLED: &str = "enabled";
pub const TENANT_STATUS_PROVISIONING_FAILED: &str = "provisioning_failed";
pub const TENANT_STATUS_DISABLED: &str = "disabled";

#[derive(Debug)]
pub struct TenantRecord {
    pub id: i64,
    pub tenant_id: String,
    pub name: String,
    pub domain: Option<String>,
    pub status: String,
    pub expire_at: Option<DateTime<Utc>>,
    pub max_users: i32,
    pub max_roles: i32,
    pub max_storage_mb: i64,
    pub max_requests_per_min: i32,
    pub session_version: i32,
    pub authorization_epoch: i32,
    pub runtime_epoch: i64,
    pub configuration_version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct TenantProvisionRequestRecord {
    pub request_token: String,
    pub admin_password_hash: String,
}

#[derive(Debug)]
pub struct TenantProductAssignmentRecord {
    pub plan_version_id: i64,
}

#[derive(Debug)]
pub struct TenantAdminRecord {
    pub password_hash: String,
}

#[derive(Debug)]
pub struct ProvisionTenantRecord {
    pub provisioning_request_token: String,
    pub tenant_id: String,
    pub name: String,
    pub domain: Option<String>,
    pub expire_at: Option<DateTime<Utc>>,
    pub max_users: i32,
    pub max_roles: i32,
    pub max_storage_mb: i64,
    pub max_requests_per_minute: i32,
    pub admin_username: String,
    pub admin_password_hash: String,
    pub enabled_capability_route_keys: Vec<String>,
    pub enabled_capability_permission_codes: Vec<String>,
    pub managed_capability_route_keys: Vec<String>,
    pub managed_capability_permission_codes: Vec<String>,
    pub default_admin_permission_codes: Vec<String>,
}

/// 租户管理用例所拥有的控制库工作单元。
pub trait TenantTransaction: Send + Sync {
    fn product(&self) -> &dyn ProductTransactionPort;

    fn authorization_mirror(&self) -> &dyn AuthorizationMirrorTransaction;

    fn lock_optional_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantRecord>>;

    fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, TenantRecord>;

    fn lock_tenant_with_limits<'a>(
        &'a self,
        tenant_id: &'a str,
        max_users: i32,
        max_roles: i32,
        max_storage_mb: i64,
    ) -> PersistenceFuture<'a, TenantRecord>;

    fn lock_provision_request<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantProvisionRequestRecord>>;

    fn provision(&self, record: ProvisionTenantRecord) -> PersistenceFuture<'_, ()>;

    fn assign_initial_product<'a>(
        &'a self,
        tenant_id: &'a str,
        plan_version_id: i64,
        changed_by: i64,
    ) -> PersistenceFuture<'a, ()>;

    fn product_assignment<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantProductAssignmentRecord>>;

    fn find_admin<'a>(
        &'a self,
        tenant_id: &'a str,
        username: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantAdminRecord>>;

    fn save_tenant(&self, tenant: TenantRecord) -> PersistenceFuture<'_, TenantRecord>;

    fn update_status<'a>(
        &'a self,
        tenant_id: &'a str,
        status: &'a str,
    ) -> PersistenceFuture<'a, ()>;

    fn create_pending<'a>(
        &'a self,
        placement: &'a TenantProvisioningPlacement,
    ) -> PersistenceFuture<'a, ()>;

    fn create_or_resume_pending<'a>(
        &'a self,
        placement: &'a TenantProvisioningPlacement,
    ) -> PersistenceFuture<'a, ()>;

    fn activate_placement<'a>(
        &'a self,
        placement: &'a TenantProvisioningPlacement,
    ) -> PersistenceFuture<'a, ()>;

    fn fail_placement<'a>(
        &'a self,
        placement: &'a TenantProvisioningPlacement,
    ) -> PersistenceFuture<'a, ()>;

    fn commit_audited(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

/// 租户管理用例所需的持久化端口。
pub trait TenantPersistencePort: Send + Sync {
    fn list(&self) -> PersistenceFuture<'_, Vec<TenantRecord>>;

    fn find<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, Option<TenantRecord>>;

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn TenantTransaction>>;
}
