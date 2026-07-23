mod cache_monitor;
pub mod server_info;

use std::{collections::BTreeMap, sync::Arc};

use axum::{Json, extract::State, response::IntoResponse};
use ryframe_common::{ApiResponse, AppResult};
use ryframe_core::{DatabaseMonitor, RedisClient};
use ryframe_macro::{get, route};
use serde::Serialize;
pub use server_info::ServerInfo;
use utoipa::ToSchema;

pub use cache_monitor::{CacheInfo, CacheKeysInfo, RedisMemoryInfo, RedisServerInfo};

#[derive(Debug, Serialize, ToSchema)]
pub struct DbPoolInfo {
    pub status: String,
    pub active_connections: Option<i64>,
    pub timestamp: String,
}

/// Monitor route state.
#[derive(Clone)]
pub struct MonitorState {
    pub database: Arc<dyn DatabaseMonitor>,
    pub redis: Option<RedisClient>,
}

/// Public metrics route. Process and dependency probes live at `/livez` and
/// `/readyz` on the root application router.
pub fn public_monitor_router(state: MonitorState) -> axum::Router {
    use axum::routing::get as axum_get;

    axum::Router::new()
        .route("/metrics", axum_get(metrics_handler))
        .with_state(state)
}

/// Sensitive monitor routes. Authentication is applied by the API composition layer.
pub fn protected_monitor_router(state: MonitorState) -> axum::Router {
    axum::Router::new()
        .merge(route!(server_info_handler))
        .merge(route!(cache_info_handler))
        .merge(route!(cache_commands_handler))
        .merge(route!(db_pool_handler))
        .with_state(state)
}

#[get("/server")]
#[perm("monitor:server:list")]
#[utoipa::path(get, path = "/api/v1/monitor/server", tag = "服务器监控",
    responses((status = 200, description = "服务器 CPU、内存、磁盘信息", body = ApiResponse<ServerInfo>)),
    security(("bearer" = [])))]
pub async fn server_info_handler(
    State(_state): State<MonitorState>,
) -> AppResult<Json<ApiResponse<ServerInfo>>> {
    Ok(Json(ApiResponse::success(
        ServerInfo::collect_async().await?,
    )))
}

#[get("/cache")]
#[perm("monitor:cache:list")]
#[utoipa::path(get, path = "/api/v1/monitor/cache", tag = "服务器监控",
    responses((status = 200, description = "缓存运行状态", body = ApiResponse<CacheInfo>)),
    security(("bearer" = [])))]
pub async fn cache_info_handler(
    State(state): State<MonitorState>,
) -> AppResult<Json<ApiResponse<CacheInfo>>> {
    let info = cache_monitor::get_cache_info(state.redis.as_ref()).await;
    Ok(Json(ApiResponse::success(info)))
}

#[get("/cache/commands")]
#[perm("monitor:cache:list")]
#[utoipa::path(get, path = "/api/v1/monitor/cache/commands", tag = "服务器监控",
    responses((status = 200, description = "Redis 命令统计", body = ApiResponse<BTreeMap<String, String>>)),
    security(("bearer" = [])))]
pub async fn cache_commands_handler(
    State(state): State<MonitorState>,
) -> AppResult<Json<ApiResponse<BTreeMap<String, String>>>> {
    let stats = match state.redis.as_ref() {
        Some(redis) => cache_monitor::get_cache_command_stats(redis)
            .await
            .unwrap_or_else(|| error_stat("failed to fetch command stats")),
        None => error_stat("Redis not configured"),
    };
    Ok(Json(ApiResponse::success(stats)))
}

#[utoipa::path(get, path = "/api/v1/monitor/metrics", tag = "服务器监控",
    responses((status = 200, description = "Prometheus 指标文本", body = String, content_type = "text/plain")))]
pub async fn metrics_handler() -> axum::response::Response {
    let text = ryframe_middleware::metrics::metrics_text();
    text_response(text, "text/plain; version=0.0.4")
}

#[get("/db-pool")]
#[perm("monitor:db-pool:list")]
#[utoipa::path(get, path = "/api/v1/monitor/db-pool", tag = "服务器监控",
    responses((status = 200, description = "数据库连接池状态", body = ApiResponse<DbPoolInfo>)),
    security(("bearer" = [])))]
pub async fn db_pool_handler(
    State(state): State<MonitorState>,
) -> AppResult<Json<ApiResponse<DbPoolInfo>>> {
    let ping_ok = state.database.ping().await;
    let active_connections = state.database.active_connections().await;

    Ok(Json(ApiResponse::success(DbPoolInfo {
        status: if ping_ok { "connected" } else { "disconnected" }.into(),
        active_connections,
        timestamp: current_timestamp(),
    })))
}

fn error_stat(message: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("error".into(), message.into())])
}

fn current_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn text_response(text: String, content_type: &'static str) -> axum::response::Response {
    ([(axum::http::header::CONTENT_TYPE, content_type)], text).into_response()
}
