use axum::Router;

use crate::{
    handlers::{product_handler, tenant_config_handler, tenant_data_handler, tenant_handler},
    state::AppState,
};

pub(in crate::router) fn router(state: AppState) -> Router {
    Router::new()
        .nest("/tenants", tenant_handler::tenant_router(state.clone()))
        .nest(
            "/tenants/{tenant_id}/config-packages",
            tenant_config_handler::config_package_router(state.clone()),
        )
        .nest(
            "/tenants/{tenant_id}/config-transfers",
            tenant_config_handler::config_transfer_router(state.clone()),
        )
        .merge(product_handler::product_router(state.clone()))
        .merge(tenant_data_handler::tenant_data_router(state))
}
