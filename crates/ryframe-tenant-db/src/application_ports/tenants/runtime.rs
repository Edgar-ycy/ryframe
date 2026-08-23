use async_trait::async_trait;
use ryframe_application::ports::tenants::{
    TenantBusinessDataState, TenantRuntimeReadPort, TenantRuntimeSnapshot,
};
use ryframe_kernel::AppResult;

use crate::{TenantDataState, TenantDatabaseRouter};

use super::super::map_error;

#[async_trait]
impl TenantRuntimeReadPort for TenantDatabaseRouter {
    async fn runtime_snapshot(&self, tenant_id: &str) -> AppResult<TenantRuntimeSnapshot> {
        let snapshot = self.runtime_snapshot(tenant_id).await.map_err(map_error)?;
        let state = match snapshot.business_data_state() {
            TenantDataState::Provisioning => TenantBusinessDataState::Provisioning,
            TenantDataState::Active => TenantBusinessDataState::Active,
            TenantDataState::Maintenance => TenantBusinessDataState::Maintenance,
            TenantDataState::Failed => TenantBusinessDataState::Failed,
        };
        Ok(TenantRuntimeSnapshot::new(
            snapshot.tenant_id().to_owned(),
            snapshot.authorization_epoch(),
            snapshot.runtime_epoch(),
            snapshot.placement_generation(),
            state,
        ))
    }
}
