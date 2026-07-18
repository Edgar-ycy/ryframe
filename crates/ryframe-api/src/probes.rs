use axum::{Json, extract::State, http::StatusCode};
use ryframe_config::RedisMode;
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
        (status = 200, description = "必要依赖可用", body = ReadinessResponse),
        (status = 503, description = "必要依赖不可用", body = ReadinessResponse)
    )
)]
pub async fn readyz(State(state): State<AppState>) -> (StatusCode, Json<ReadinessResponse>) {
    let dependency_timeout = std::time::Duration::from_secs(2);
    let mysql = tokio::time::timeout(dependency_timeout, state.monitor.database.ping());
    // Detaching the storage task on timeout lets its readiness canary reach the
    // delete step instead of cancelling between put/get/delete and leaking an
    // object under `.ryframe-readiness/`.
    let file_service = state.services.file.clone();
    let storage_task = tokio::spawn(async move { file_service.check_storage().await });
    let storage = tokio::time::timeout(dependency_timeout, storage_task);
    let redis_required = state
        .config
        .redis
        .as_ref()
        .is_some_and(|config| config.mode == RedisMode::Required);
    let redis = tokio::time::timeout(dependency_timeout, async {
        match &state.redis {
            Some(redis) => redis.ping().await.is_ok(),
            None => false,
        }
    });
    let (mysql_result, redis_result, storage_result) = tokio::join!(mysql, redis, storage);
    let mysql_ok = matches!(mysql_result, Ok(true));
    let redis_reachable = matches!(redis_result, Ok(true));
    let redis_ok = !redis_required || redis_reachable;
    let storage_ok = matches!(storage_result, Ok(Ok(Ok(()))));

    if !mysql_ok {
        ryframe_middleware::metrics::record_readiness_failure("mysql");
    }
    if !redis_ok {
        ryframe_middleware::metrics::record_readiness_failure("redis");
    }
    ryframe_middleware::metrics::set_redis_degraded_state("readiness", !redis_reachable);
    if !storage_ok {
        ryframe_middleware::metrics::record_readiness_failure("object_storage");
    }

    let ready = mysql_ok && redis_ok && storage_ok;
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(ReadinessResponse {
            status: if ready { "ready" } else { "not_ready" },
            mysql: if mysql_ok { "up" } else { "down" },
            redis: if redis_reachable {
                "up"
            } else if redis_required {
                "down"
            } else {
                "optional_degraded"
            },
            object_storage: if storage_ok { "up" } else { "down" },
        }),
    )
}
