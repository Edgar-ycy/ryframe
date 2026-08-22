use ryframe_application::ports::tenants::{
    TenantProvisioningFuture, TenantProvisioningPlacement, TenantProvisioningPort,
};
use ryframe_kernel::{AppError, AppResult};

use crate::{PendingTenantDataPlacement, TenantDataError, TenantDatabaseRouter};

impl TenantProvisioningPort for TenantDatabaseRouter {
    fn prepare(
        &self,
        tenant_id: String,
        target_key: String,
        generation: i64,
        switch_token: String,
    ) -> AppResult<TenantProvisioningPlacement> {
        self.prepare_provisioning(tenant_id, target_key, generation, switch_token)
            .map(to_application_placement)
            .map_err(map_error)
    }

    fn provision_fence<'a>(
        &'a self,
        placement: &'a TenantProvisioningPlacement,
    ) -> TenantProvisioningFuture<'a> {
        Box::pin(async move {
            self.provision_pending_fence(&to_infrastructure_placement(placement))
                .await
                .map_err(map_error)
        })
    }
}

pub fn to_application_placement(
    placement: PendingTenantDataPlacement,
) -> TenantProvisioningPlacement {
    TenantProvisioningPlacement {
        tenant_id: placement.tenant_id,
        target_key: placement.current_target_key,
        generation: placement.placement_generation,
        switch_token: placement.switch_token,
    }
}

pub fn to_infrastructure_placement(
    placement: &TenantProvisioningPlacement,
) -> PendingTenantDataPlacement {
    PendingTenantDataPlacement {
        tenant_id: placement.tenant_id.clone(),
        current_target_key: placement.target_key.clone(),
        placement_generation: placement.generation,
        switch_token: placement.switch_token.clone(),
    }
}

fn map_error(error: TenantDataError) -> AppError {
    let message = error.to_string();
    match error {
        TenantDataError::InvalidConfiguration(_)
        | TenantDataError::InvalidTenantId(_)
        | TenantDataError::UnknownTarget { .. } => AppError::Validation(message),
        TenantDataError::PlacementUnavailable { .. }
        | TenantDataError::InvalidPlacement { .. }
        | TenantDataError::FenceRejected { .. }
        | TenantDataError::DedicatedTargetOccupied { .. } => {
            AppError::TenantOperationConflict(message)
        }
        TenantDataError::StalePlacementGeneration { .. } => {
            AppError::StalePlacementGeneration(message)
        }
        TenantDataError::TenantDataMaintenance { .. } => {
            AppError::TenantDataMaintenance(message, 5)
        }
        TenantDataError::TargetUnavailable { .. }
        | TenantDataError::PoolCapacityExhausted { .. }
        | TenantDataError::ConnectionBudgetExhausted { .. } => {
            AppError::TenantDataTargetUnavailable(message, 5)
        }
    }
}
