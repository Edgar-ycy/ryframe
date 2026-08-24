use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct ProvisionTenantCommand {
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
