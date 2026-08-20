use ryframe_application::{
    TenantBusinessDataState, TenantRuntimeReadFuture, TenantRuntimeReadPort, TenantRuntimeSnapshot,
};

use crate::{TenantDataState, TenantDatabaseRouter};

use super::map_error;

impl TenantRuntimeReadPort for TenantDatabaseRouter {
    fn runtime_snapshot<'a>(&'a self, tenant_id: &'a str) -> TenantRuntimeReadFuture<'a> {
        Box::pin(async move {
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
        })
    }
}
