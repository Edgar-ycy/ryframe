use axum::Router;

use crate::{
    handlers::{product_handler, tenant_data_handler, tenant_handler},
    state::AppState,
};

pub(in crate::router) fn router(state: AppState) -> Router {
    Router::new()
        .nest("/tenants", tenant_handler::tenant_router(state.clone()))
        .merge(product_handler::product_router(state.clone()))
        .merge(tenant_data_handler::tenant_data_router(state))
}
