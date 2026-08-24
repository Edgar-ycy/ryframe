use axum::Router;

use crate::{
    handlers::{common_handler, export_handler},
    state::AppState,
};

pub(in crate::router) fn upload(state: AppState) -> Router {
    common_handler::upload_router(state)
}

pub(in crate::router) fn download(state: AppState) -> Router {
    common_handler::download_router(state)
}

pub(in crate::router) fn exports(state: AppState) -> Router {
    export_handler::export_router(state)
}

pub(in crate::router) fn notification_read(state: AppState) -> Router {
    export_handler::notification_read_router(state)
}
