mod cache_monitor;
mod readiness;
pub mod server_info;

#[doc(hidden)]
pub mod __macro_support {
    use std::{future::Future, pin::Pin, sync::Arc};

    use axum::{
        extract::Request,
        middleware::{self, Next},
        response::{IntoResponse, Response},
        routing::MethodRouter,
    };
    use ryframe_auth::{RequestPrincipal, permission::check_permission};
    use ryframe_http::HttpAppError;
    use ryframe_kernel::AppError;

    type PermissionFuture = Pin<Box<dyn Future<Output = Result<Response, Response>> + Send>>;

    fn require_permission(
        permission: &'static str,
    ) -> impl Fn(Request, Next) -> PermissionFuture + Clone {
        move |request: Request, next: Next| {
            Box::pin(async move {
                let principal = request
                    .extensions()
                    .get::<Arc<RequestPrincipal>>()
                    .ok_or_else(|| {
                        HttpAppError::from(AppError::Authentication("未认证，请先登录".into()))
                            .into_response()
                    })?;
                check_permission(principal, permission)
                    .map_err(|error| HttpAppError::from(error).into_response())?;
                Ok(next.run(request).await)
            })
        }
    }

    pub fn perm_route<S>(route: MethodRouter<S>, permission: &'static str) -> MethodRouter<S>
    where
        S: Clone + Send + Sync + 'static,
    {
        route.route_layer(middleware::from_fn(require_permission(permission)))
    }
}

use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use ryframe_adapters::{DatabaseMonitor, RedisClient};
use ryframe_http::{ApiResponse, HttpResult};
use ryframe_macro::{get, route};
use serde::Serialize;
pub use server_info::{ServerInfo, ServerInfoSampler};
use utoipa::ToSchema;

pub use cache_monitor::{
    CacheCommandStats, CacheCommandStatsStatus, CacheInfo, CacheKeysInfo, RedisMemoryInfo,
    RedisServerInfo,
};
pub use readiness::{DependencyHealthCache, DependencyHealthSnapshot, DependencyStatus};

#[derive(Debug, Serialize, ToSchema)]
pub struct DbPoolInfo {
    pub status: String,
    pub active_connections: Option<i64>,
    pub timestamp: String,
}

/// 监控路由状态。
#[derive(Clone)]
pub struct MonitorState {
    pub database: Arc<dyn DatabaseMonitor>,
    pub redis: Option<RedisClient>,
    /// Redis 是否已在配置中启用。客户端缺失时，用于区分显式未配置和运行时故障。
    pub redis_configured: bool,
    pub readiness: DependencyHealthCache,
    pub metrics_bearer_token: Arc<str>,
    pub server_info: ServerInfoSampler,
}

/// 公开指标路由。进程和依赖探针位于根应用路由的 `/livez` 与
/// `/readyz`。
pub fn public_monitor_router(state: MonitorState) -> axum::Router {
    use axum::routing::get as axum_get;

    axum::Router::new()
        .route("/metrics", axum_get(metrics_handler))
        .with_state(state)
}

/// 敏感监控路由。认证由 API 组合层施加。
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
    State(state): State<MonitorState>,
) -> HttpResult<Json<ApiResponse<ServerInfo>>> {
    Ok(Json(ApiResponse::success(state.server_info.latest())))
}

#[get("/cache")]
#[perm("monitor:cache:list")]
#[utoipa::path(get, path = "/api/v1/monitor/cache", tag = "服务器监控",
    responses((status = 200, description = "缓存运行状态", body = ApiResponse<CacheInfo>)),
    security(("bearer" = [])))]
pub async fn cache_info_handler(
    State(state): State<MonitorState>,
) -> HttpResult<Json<ApiResponse<CacheInfo>>> {
    let info = cache_monitor::get_cache_info(state.redis.as_ref()).await;
    Ok(Json(ApiResponse::success(info)))
}

#[get("/cache/commands")]
#[perm("monitor:cache:list")]
#[utoipa::path(get, path = "/api/v1/monitor/cache/commands", tag = "服务器监控",
    responses((status = 200, description = "Redis 命令统计", body = ApiResponse<CacheCommandStats>)),
    security(("bearer" = [])))]
pub async fn cache_commands_handler(
    State(state): State<MonitorState>,
) -> HttpResult<Json<ApiResponse<CacheCommandStats>>> {
    let stats = match state.redis.as_ref() {
        Some(redis) => cache_monitor::get_cache_command_stats(redis).await,
        None if state.redis_configured => CacheCommandStats::unavailable(),
        None => CacheCommandStats::not_configured(),
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
) -> HttpResult<Json<ApiResponse<DbPoolInfo>>> {
    let ping_ok = state.database.ping().await;
    let active_connections = state.database.active_connections().await;

    Ok(Json(ApiResponse::success(DbPoolInfo {
        status: if ping_ok { "connected" } else { "disconnected" }.into(),
        active_connections,
        timestamp: current_timestamp(),
    })))
}

fn current_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn text_response(text: String, content_type: &'static str) -> axum::response::Response {
    ([(header::CONTENT_TYPE, content_type)], text).into_response()
}
