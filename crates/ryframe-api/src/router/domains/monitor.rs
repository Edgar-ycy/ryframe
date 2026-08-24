use axum::Router;

use crate::{
    handlers::{job_handler, overview_handler, retention_handler, schedule_handler},
    monitor::MonitorState,
    state::AppState,
};

pub(in crate::router) fn public(state: MonitorState) -> Router {
    crate::monitor::public_router(state)
}

pub(in crate::router) fn protected(state: AppState, monitor_state: MonitorState) -> Router {
    let mut router = crate::monitor::protected_router(monitor_state)
        .merge(ryframe_macro::route!(super::super::runtime_status).with_state(state.clone()))
        .merge(overview_handler::overview_router(state.clone()))
        .merge(job_handler::job_router(state.clone()))
        .merge(retention_handler::retention_router(state.clone()));
    if state.settings.jobs.scheduler_enabled {
        router = router.merge(schedule_handler::schedule_router(state));
    }
    router
}
