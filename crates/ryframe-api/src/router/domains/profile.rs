use axum::{Router, middleware::from_fn_with_state};

use crate::{handlers::service_delegation_profile_handler, state::AppState};

use super::super::{CapabilityGuardState, capability_guard};

pub(in crate::router) fn service_delegations(state: AppState) -> Router {
    service_delegation_profile_handler::service_delegation_profile_router(state.clone()).layer(
        from_fn_with_state(
            CapabilityGuardState::new(
                state,
                ryframe_application::system::SERVICE_ACCOUNTS_CAPABILITY,
            ),
            capability_guard,
        ),
    )
}
