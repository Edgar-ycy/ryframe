mod cache_monitor;
pub mod server_info;

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
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
    pub metrics_bearer_token: Arc<str>,
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
    responses(
        (status = 200, description = "Prometheus 指标文本", body = String, content_type = "text/plain"),
        (status = 401, description = "缺少或无效的监控 Bearer Token")
    ))]
pub async fn metrics_handler(
    State(state): State<MonitorState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if !state.metrics_bearer_token.is_empty()
        && !has_valid_metrics_token(&headers, &state.metrics_bearer_token)
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let text = ryframe_middleware::metrics::metrics_text();
    text_response(text, "text/plain; version=0.0.4")
}

fn has_valid_metrics_token(headers: &HeaderMap, expected: &str) -> bool {
    let Some(actual) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    constant_time_eq(actual.as_bytes(), expected.as_bytes())
}

fn constant_time_eq(actual: &[u8], expected: &[u8]) -> bool {
    let max_len = actual.len().max(expected.len());
    let mut difference = actual.len() ^ expected.len();
    for index in 0..max_len {
        difference |= usize::from(actual.get(index).copied().unwrap_or(0))
            ^ usize::from(expected.get(index).copied().unwrap_or(0));
    }
    difference == 0
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

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};

    use super::{constant_time_eq, has_valid_metrics_token};

    #[test]
    fn metrics_token_requires_exact_bearer_value() {
        let mut headers = HeaderMap::new();
        assert!(!has_valid_metrics_token(&headers, "expected-secret"));

        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer wrong-secret"),
        );
        assert!(!has_valid_metrics_token(&headers, "expected-secret"));

        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer expected-secret"),
        );
        assert!(has_valid_metrics_token(&headers, "expected-secret"));
    }

    #[test]
    fn constant_time_comparison_rejects_prefixes_and_length_mismatches() {
        assert!(constant_time_eq(b"same", b"same"));
        assert!(!constant_time_eq(b"same", b"same-but-longer"));
        assert!(!constant_time_eq(b"same-but-longer", b"same"));
        assert!(!constant_time_eq(b"same", b"diff"));
    }
}
