use crate::RequestPrincipal;
use crate::http::{ApiResponse, HttpResult};
use axum::{
    Json, Router,
    extract::{Query, State},
};
use ryframe_application::system::OverviewRange;
use ryframe_macro::{get, route};

use crate::{
    dto::{
        overview_dto::OverviewTrendQuery,
        public_dto::{
            MonitorOverviewDatabasePoolVo, MonitorOverviewDependenciesVo,
            MonitorOverviewDependencyVo, MonitorOverviewSystemVo, MonitorOverviewTrendsVo,
            MonitorOverviewVo,
        },
    },
    state::AppState,
};

pub fn overview_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(overview))
        .merge(route!(trends))
        .with_state(state)
}

#[get("/overview")]
#[perm("monitor:overview:list")]
#[utoipa::path(
    get,
    path = "/api/v1/monitor/overview",
    tag = "运维总览",
    responses((status = 200, description = "当前租户运维快照", body = ApiResponse<MonitorOverviewVo>)),
    security(("bearer" = []))
)]
pub(crate) async fn overview(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> HttpResult<Json<ApiResponse<MonitorOverviewVo>>> {
    let core = state
        .services
        .overview
        .snapshot(&current_user)
        .await
        .map_err(crate::http::HttpAppError::from)?;
    let calculated_at = core.calculated_at;
    let topology = state.monitor.database.topology_health().await;
    let active_connections = state.monitor.database.active_connections().await;
    let readiness = state.monitor.readiness.snapshot();
    let storage_available = state.services.file.check_storage().await.is_ok();
    let redis_configured = state.settings.redis_configured;
    let database_status = if topology.primary_healthy {
        if topology.replicas.iter().all(|node| node.healthy)
            && topology.sources.iter().all(|node| node.healthy)
        {
            "up"
        } else {
            "degraded"
        }
    } else {
        "down"
    };
    let redis_status = if redis_configured {
        readiness.redis.as_str()
    } else {
        "disabled"
    };
    let messaging_status = if !state.settings.messaging.enabled {
        "disabled"
    } else if redis_configured && redis_status != "up" {
        "degraded"
    } else {
        "up"
    };

    Ok(Json(ApiResponse::success(MonitorOverviewVo {
        calculated_at,
        dependencies: MonitorOverviewDependenciesVo {
            database: MonitorOverviewDependencyVo {
                status: database_status.to_owned(),
                configured: true,
                detail: None,
            },
            redis: MonitorOverviewDependencyVo {
                status: redis_status.to_owned(),
                configured: redis_configured,
                detail: None,
            },
            object_storage: MonitorOverviewDependencyVo {
                status: if storage_available { "up" } else { "down" }.to_owned(),
                configured: true,
                detail: Some(state.settings.object_storage.backend.clone()),
            },
            messaging: MonitorOverviewDependencyVo {
                status: messaging_status.to_owned(),
                configured: state.settings.messaging.enabled,
                detail: None,
            },
        },
        system: MonitorOverviewSystemVo::from(state.monitor.server_info.latest()),
        database_pool: MonitorOverviewDatabasePoolVo {
            status: database_status.to_owned(),
            active_connections,
        },
        jobs: core.into(),
    })))
}

#[get("/overview/trends")]
#[perm("monitor:overview:list")]
#[utoipa::path(
    get,
    path = "/api/v1/monitor/overview/trends",
    tag = "运维总览",
    params(OverviewTrendQuery),
    responses((status = 200, description = "当前租户补零后的 UTC 趋势桶", body = ApiResponse<MonitorOverviewTrendsVo>)),
    security(("bearer" = []))
)]
pub(crate) async fn trends(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<OverviewTrendQuery>,
) -> HttpResult<Json<ApiResponse<MonitorOverviewTrendsVo>>> {
    let range = OverviewRange::parse(query.range.trim())?;
    state
        .services
        .overview
        .trends(&current_user, range)
        .await
        .map_err(crate::http::HttpAppError::from)
        .map(MonitorOverviewTrendsVo::from)
        .map(ApiResponse::success)
        .map(Json)
}
