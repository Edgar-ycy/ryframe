use std::collections::HashSet;

#[derive(Debug)]
pub struct ServiceAccountPermissionSnapshot {
    pub user_permissions: HashSet<String>,
    pub account_permissions: HashSet<String>,
}

#[derive(Debug)]
pub struct ServiceDelegationTargetRecord {
    pub account_id: i64,
    pub code: String,
    pub name: String,
    pub permission_codes: HashSet<String>,
}

#[derive(Debug)]
pub struct ServiceDelegationTargetSet {
    pub user_permissions: HashSet<String>,
    pub accounts: Vec<ServiceDelegationTargetRecord>,
}

#[async_trait::async_trait]
pub trait ServiceAccountAuthorizationReadPort: Send + Sync {
    async fn permission_snapshot<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        account_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ServiceAccountPermissionSnapshot>>;

    async fn delegation_targets<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        limit: u64,
    ) -> ryframe_kernel::AppResult<ServiceDelegationTargetSet>;
}
