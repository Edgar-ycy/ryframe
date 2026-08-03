use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;
use utoipa::ToSchema;

use crate::AppState;

#[derive(Debug, Serialize, ToSchema)]
pub struct LivenessResponse {
    status: &'static str,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReadinessResponse {
    status: &'static str,
    mysql: &'static str,
    redis: &'static str,
    object_storage: &'static str,
}

#[utoipa::path(
    get,
    path = "/livez",
    tag = "运行探针",
    responses((status = 200, description = "进程存活", body = LivenessResponse))
)]
pub async fn livez() -> (StatusCode, Json<LivenessResponse>) {
    (StatusCode::OK, Json(LivenessResponse { status: "alive" }))
}

#[utoipa::path(
    get,
    path = "/readyz",
    tag = "运行探针",
    responses(
        (status = 200, description = "后台依赖快照有效且必要依赖可用", body = ReadinessResponse),
        (status = 503, description = "后台依赖快照过期或必要依赖不可用", body = ReadinessResponse)
    )
)]
pub async fn readyz(State(state): State<AppState>) -> (StatusCode, Json<ReadinessResponse>) {
    let snapshot = state.monitor.readiness.snapshot();
    let ready = snapshot.is_ready();
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(ReadinessResponse {
            status: if ready { "ready" } else { "not_ready" },
            mysql: snapshot.mysql.as_str(),
            redis: snapshot.redis.as_str(),
            object_storage: snapshot.object_storage.as_str(),
        }),
    )
}
