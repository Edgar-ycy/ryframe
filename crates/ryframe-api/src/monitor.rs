//! 系统监控的 HTTP 状态、路由、处理器与响应模型。

use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};

mod readiness;

pub use readiness::{DependencyHealthCache, DependencyHealthSnapshot, DependencyStatus};

use crate::http::{ApiResponse, HttpResult};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::get as axum_get,
};
use ryframe_macro::{get, route};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabaseNodeHealth {
    pub name: String,
    pub healthy: bool,
    pub consecutive_failures: usize,
    pub consecutive_successes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabaseTopologyHealth {
    pub primary_healthy: bool,
    pub replicas: Vec<DatabaseNodeHealth>,
    pub sources: Vec<DatabaseNodeHealth>,
}

pub type DatabasePingFuture<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;
pub type DatabaseConnectionCountFuture<'a> = Pin<Box<dyn Future<Output = Option<i64>> + Send + 'a>>;
pub type DatabaseTopologyFuture<'a> =
    Pin<Box<dyn Future<Output = DatabaseTopologyHealth> + Send + 'a>>;

/// HTTP 监控所需的只读数据库探针，由组合根提供具体实现。
pub trait DatabaseMonitor: Send + Sync {
    fn ping(&self) -> DatabasePingFuture<'_>;
    fn active_connections(&self) -> DatabaseConnectionCountFuture<'_>;
    fn topology_health(&self) -> DatabaseTopologyFuture<'_>;
}

pub type CacheInfoFuture<'a> = Pin<Box<dyn Future<Output = CacheInfo> + Send + 'a>>;
pub type CacheCommandStatsFuture<'a> = Pin<Box<dyn Future<Output = CacheCommandStats> + Send + 'a>>;

/// HTTP 缓存监控所需的只读端口。
pub trait CacheMonitor: Send + Sync {
    fn info(&self) -> CacheInfoFuture<'_>;
    fn command_stats(&self) -> CacheCommandStatsFuture<'_>;
}

/// HTTP 系统监控所需的最近一次采样读取端口。
pub trait ServerInfoMonitor: Send + Sync {
    fn latest(&self) -> ServerInfo;
}

/// 监控路由状态。
#[derive(Clone)]
pub struct MonitorState {
    pub database: Arc<dyn DatabaseMonitor>,
    pub cache: Arc<dyn CacheMonitor>,
    pub readiness: DependencyHealthCache,
    pub metrics_bearer_token: Arc<str>,
    pub server_info: Arc<dyn ServerInfoMonitor>,
}

/// 公开指标路由。进程和依赖探针位于根应用路由的 `/livez` 与 `/readyz`。
pub(crate) fn public_router(state: MonitorState) -> Router {
    Router::new()
        .route("/metrics", axum_get(metrics_handler))
        .with_state(state)
}

/// 需要认证的监控路由。
pub(crate) fn protected_router(state: MonitorState) -> Router {
    Router::new()
        .merge(route!(server_info_handler))
        .merge(route!(cache_info_handler))
        .merge(route!(cache_commands_handler))
        .merge(route!(db_pool_handler))
        .with_state(state)
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ServerInfo {
    /// 操作系统。
    pub os: String,
    /// 主机名。
    pub hostname: String,
    /// CPU 核心数。
    pub cpu_cores: usize,
    /// CPU 使用率（百分比）。
    pub cpu_usage: f32,
    /// 总内存（GB）。
    pub total_memory: f64,
    /// 已用内存（GB）。
    pub used_memory: f64,
    /// 内存使用率（百分比）。
    pub memory_usage: f32,
    /// 进程 PID。
    pub pid: u32,
    /// 系统运行时长（秒）。
    pub uptime: u64,
}

/// 缓存信息响应
#[derive(Debug, Serialize, ToSchema)]
pub struct CacheInfo {
    /// Redis 是否可用
    pub available: bool,
    /// 缓存模式: "redis" 或 "memory"
    pub mode: String,
    /// Redis 服务器信息（仅 Redis 模式）
    pub server: Option<RedisServerInfo>,
    /// 键统计
    pub keys: CacheKeysInfo,
    /// 内存信息
    pub memory: Option<RedisMemoryInfo>,
}

/// Redis 服务器基本信息
#[derive(Debug, Serialize, ToSchema)]
pub struct RedisServerInfo {
    /// Redis 版本
    pub version: String,
    /// 运行模式
    pub mode: String,
    /// 操作系统
    pub os: String,
    /// 运行天数
    pub uptime_days: u64,
    /// 连接数
    pub connected_clients: u64,
}

/// 缓存键统计
#[derive(Debug, Serialize, ToSchema)]
pub struct CacheKeysInfo {
    /// 当前数据库键总数
    pub total_keys: u64,
    /// 在线用户会话数
    pub online_users: u64,
    /// 验证码数
    pub captchas: u64,
    /// 限流计数器数
    pub rate_limits: u64,
    /// 字典缓存数
    pub dict_cache: u64,
    /// 配置缓存数
    pub config_cache: u64,
}

/// Redis 内存信息
#[derive(Debug, Serialize, ToSchema)]
pub struct RedisMemoryInfo {
    /// 已用内存（人类可读）
    pub used_memory_human: String,
    /// 内存峰值（人类可读）
    pub used_memory_peak_human: String,
    /// 内存碎片率
    pub mem_fragmentation_ratio: f64,
    /// 已用内存（字节）
    pub used_memory: u64,
}

/// Redis 命令统计查询状态。
///
/// `not_configured` 表示当前实例没有启用 Redis；`unavailable` 表示 Redis
/// 已配置但连接或查询失败。两种情况下均返回空的 `commands`，避免让调用方
/// 将错误文本误当作命令名称渲染。
#[derive(Debug, Clone, Copy, Serialize, ToSchema, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CacheCommandStatsStatus {
    Available,
    NotConfigured,
    Unavailable,
}

/// Redis 命令统计响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct CacheCommandStats {
    pub status: CacheCommandStatsStatus,
    pub commands: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DbPoolInfo {
    pub status: String,
    pub active_connections: Option<i64>,
    pub timestamp: String,
}

#[get("/server")]
#[perm("monitor:server:list")]
#[utoipa::path(get, path = "/api/v1/monitor/server", tag = "服务器监控",
    responses((status = 200, description = "服务器 CPU、内存、磁盘信息", body = ApiResponse<ServerInfo>)),
    security(("bearer" = [])))]
pub(crate) async fn server_info_handler(
    State(state): State<MonitorState>,
) -> HttpResult<Json<ApiResponse<ServerInfo>>> {
    Ok(Json(ApiResponse::success(state.server_info.latest())))
}

#[get("/cache")]
#[perm("monitor:cache:list")]
#[utoipa::path(get, path = "/api/v1/monitor/cache", tag = "服务器监控",
    responses((status = 200, description = "缓存运行状态", body = ApiResponse<CacheInfo>)),
    security(("bearer" = [])))]
pub(crate) async fn cache_info_handler(
    State(state): State<MonitorState>,
) -> HttpResult<Json<ApiResponse<CacheInfo>>> {
    Ok(Json(ApiResponse::success(state.cache.info().await)))
}

#[get("/cache/commands")]
#[perm("monitor:cache:list")]
#[utoipa::path(get, path = "/api/v1/monitor/cache/commands", tag = "服务器监控",
    responses((status = 200, description = "Redis 命令统计", body = ApiResponse<CacheCommandStats>)),
    security(("bearer" = [])))]
pub(crate) async fn cache_commands_handler(
    State(state): State<MonitorState>,
) -> HttpResult<Json<ApiResponse<CacheCommandStats>>> {
    Ok(Json(ApiResponse::success(
        state.cache.command_stats().await,
    )))
}

#[utoipa::path(get, path = "/api/v1/monitor/metrics", tag = "服务器监控",
    responses(
        (status = 200, description = "Prometheus 指标文本", body = String, content_type = "text/plain"),
        (status = 401, description = "缺少或无效的监控 Bearer Token")
    ))]
pub(crate) async fn metrics_handler(
    State(state): State<MonitorState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if !state.metrics_bearer_token.is_empty()
        && !has_valid_metrics_token(&headers, &state.metrics_bearer_token)
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    text_response(crate::metrics::metrics_text(), "text/plain; version=0.0.4")
}

#[get("/db-pool")]
#[perm("monitor:db-pool:list")]
#[utoipa::path(get, path = "/api/v1/monitor/db-pool", tag = "服务器监控",
    responses((status = 200, description = "数据库连接池状态", body = ApiResponse<DbPoolInfo>)),
    security(("bearer" = [])))]
pub(crate) async fn db_pool_handler(
    State(state): State<MonitorState>,
) -> HttpResult<Json<ApiResponse<DbPoolInfo>>> {
    let ping_ok = state.database.ping().await;
    let active_connections = state.database.active_connections().await;

    Ok(Json(ApiResponse::success(DbPoolInfo {
        status: if ping_ok { "connected" } else { "disconnected" }.into(),
        active_connections,
        timestamp: chrono::Utc::now().to_rfc3339(),
    })))
}

fn has_valid_metrics_token(headers: &HeaderMap, expected: &str) -> bool {
    let Some(actual) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    ryframe_auth::constant_time_eq(actual.as_bytes(), expected.as_bytes())
}

fn text_response(text: String, content_type: &'static str) -> axum::response::Response {
    ([(header::CONTENT_TYPE, content_type)], text).into_response()
}
