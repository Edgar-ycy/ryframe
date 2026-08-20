use std::sync::Arc;

use ryframe_application::{
    TenantProvisioningFuture, TenantProvisioningPlacement, TenantProvisioningPort,
};
use ryframe_kernel::{AppError, AppResult};
use ryframe_tenant_db::{
    PendingTenantDataPlacement, TenantDataError, TenantDataPlacementRepository,
    TenantDatabaseRouter,
};
use sea_orm::DatabaseTransaction;

struct TenantProvisioningBridge {
    router: Arc<TenantDatabaseRouter>,
}

impl TenantProvisioningPort for TenantProvisioningBridge {
    fn prepare(
        &self,
        tenant_id: String,
        target_key: String,
        generation: i64,
        switch_token: String,
    ) -> AppResult<TenantProvisioningPlacement> {
        self.router
            .prepare_provisioning(tenant_id, target_key, generation, switch_token)
            .map(to_application_placement)
            .map_err(map_error)
    }

    fn create_pending<'a>(
        &'a self,
        transaction: &'a DatabaseTransaction,
        placement: &'a TenantProvisioningPlacement,
    ) -> TenantProvisioningFuture<'a> {
        Box::pin(async move {
            TenantDataPlacementRepository
                .create_pending(transaction, &to_infrastructure_placement(placement))
                .await
                .map_err(map_error)
        })
    }

    fn create_or_resume_pending<'a>(
        &'a self,
        transaction: &'a DatabaseTransaction,
        placement: &'a TenantProvisioningPlacement,
    ) -> TenantProvisioningFuture<'a> {
        Box::pin(async move {
            TenantDataPlacementRepository
                .create_or_resume_pending(transaction, &to_infrastructure_placement(placement))
                .await
                .map_err(map_error)
        })
    }

    fn provision_fence<'a>(
        &'a self,
        placement: &'a TenantProvisioningPlacement,
    ) -> TenantProvisioningFuture<'a> {
        Box::pin(async move {
            self.router
                .provision_pending_fence(&to_infrastructure_placement(placement))
                .await
                .map_err(map_error)
        })
    }

    fn activate<'a>(
        &'a self,
        transaction: &'a DatabaseTransaction,
        placement: &'a TenantProvisioningPlacement,
    ) -> TenantProvisioningFuture<'a> {
        Box::pin(async move {
            TenantDataPlacementRepository
                .activate(transaction, &to_infrastructure_placement(placement))
                .await
                .map_err(map_error)
        })
    }

    fn fail<'a>(
        &'a self,
        transaction: &'a DatabaseTransaction,
        placement: &'a TenantProvisioningPlacement,
    ) -> TenantProvisioningFuture<'a> {
        Box::pin(async move {
            TenantDataPlacementRepository
                .fail(transaction, &to_infrastructure_placement(placement))
                .await
                .map_err(map_error)
        })
    }
}

pub fn port(router: Arc<TenantDatabaseRouter>) -> Arc<dyn TenantProvisioningPort> {
    Arc::new(TenantProvisioningBridge { router })
}

fn to_application_placement(placement: PendingTenantDataPlacement) -> TenantProvisioningPlacement {
    TenantProvisioningPlacement {
        tenant_id: placement.tenant_id,
        target_key: placement.current_target_key,
        generation: placement.placement_generation,
        switch_token: placement.switch_token,
    }
}

fn to_infrastructure_placement(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_mapping_preserves_all_fields() {
        let application = TenantProvisioningPlacement {
            tenant_id: "tenant-a".into(),
            target_key: "primary".into(),
            generation: 3,
            switch_token: "token".into(),
        };
        let infrastructure = to_infrastructure_placement(&application);
        assert_eq!(to_application_placement(infrastructure), application);
    }
}
