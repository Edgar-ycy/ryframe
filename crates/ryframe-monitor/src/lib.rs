mod cache_monitor;
pub mod server_info;

use axum::{Json, extract::State};
use ryframe_auth::middleware::AuthState;
use ryframe_common::{ApiResponse, AppResult};
use ryframe_core::RedisClient;
use sea_orm::DatabaseConnection;
pub use server_info::ServerInfo;

/// 监控路由状态
#[derive(Clone)]
pub struct MonitorState {
    pub db: DatabaseConnection,
    pub redis: Option<RedisClient>,
}

/// 监控路由
///
/// `auth_state` 为 `Some` 时，敏感路由（/server, /cache, /db-pool）需认证。
/// `/health` 和 `/metrics` 始终公开。
pub fn monitor_router(state: MonitorState, auth_state: Option<AuthState>) -> axum::Router {
    use axum::{middleware, routing::get};

    // 公开路由（健康检查 + Prometheus 指标）
    let public = axum::Router::new()
        .route("/health", get(health_check_handler))
        .route("/metrics", get(metrics_handler));

    // 受保护路由（服务器信息、缓存、DB 连接池）
    let mut protected = axum::Router::new()
        .route("/server", get(server_info_handler))
        .route("/cache", get(cache_info_handler))
        .route("/cache/commands", get(cache_commands_handler))
        .route("/db-pool", get(db_pool_handler));

    if let Some(auth) = auth_state {
        protected = protected.route_layer(middleware::from_fn_with_state(
            auth,
            ryframe_auth::middleware::auth_middleware,
        ));
    }

    axum::Router::new()
        .merge(public)
        .merge(protected)
        .with_state(state)
}

/// 服务器信息
async fn server_info_handler() -> AppResult<Json<ApiResponse<ServerInfo>>> {
    Ok(Json(ApiResponse::success(ServerInfo::collect())))
}

/// 增强健康检查（含 DB + Redis 连通性）
async fn health_check_handler(
    State(state): State<MonitorState>,
) -> AppResult<Json<ApiResponse<serde_json::Value>>> {
    let db_ok = ryframe_db::connection::ping(&state.db).await.is_ok();
    let redis_ok = match state.redis.as_ref() {
        Some(r) => r.ping().await.is_ok(),
        None => false,
    };
    let redis_configured = state.redis.is_some();
    let all_ok = db_ok && (!redis_configured || redis_ok);

    Ok(Json(ApiResponse::success(serde_json::json!({
        "status": if all_ok { "UP" } else { "DOWN" },
        "database": if db_ok { "connected" } else { "disconnected" },
        "redis": if redis_configured {
            if redis_ok { "connected" } else { "disconnected" }
        } else {
            "not_configured"
        },
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))))
}

/// 缓存信息
async fn cache_info_handler(
    State(state): State<MonitorState>,
) -> AppResult<Json<ApiResponse<cache_monitor::CacheInfo>>> {
    let info = cache_monitor::get_cache_info(state.redis.as_ref()).await;
    Ok(Json(ApiResponse::success(info)))
}

/// 缓存命令统计
async fn cache_commands_handler(
    State(state): State<MonitorState>,
) -> AppResult<Json<ApiResponse<serde_json::Value>>> {
    match state.redis.as_ref() {
        Some(redis) => {
            let stats = cache_monitor::get_cache_command_stats(redis).await;
            Ok(Json(ApiResponse::success(stats.unwrap_or(
                serde_json::json!({"error": "无法获取命令统计"}),
            ))))
        }
        None => Ok(Json(ApiResponse::success(
            serde_json::json!({"error": "Redis 未配置"}),
        ))),
    }
}

/// Prometheus Metrics 端点
async fn metrics_handler() -> axum::response::Response {
    let text = ryframe_middleware::metrics::metrics_text();
    axum::response::Response::builder()
        .header("Content-Type", "text/plain; version=0.0.4")
        .body(axum::body::Body::from(text))
        .unwrap()
}

/// 数据库连接池状态
async fn db_pool_handler(
    State(state): State<MonitorState>,
) -> AppResult<Json<ApiResponse<serde_json::Value>>> {
    use sea_orm::{FromQueryResult, Statement};

    let backend = state.db.get_database_backend();
    let ping_ok = ryframe_db::connection::ping(&state.db).await.is_ok();

    // 尝试查询活跃连接数
    let active_connections = match backend {
        sea_orm::DatabaseBackend::MySql => {
            let sql = "SHOW STATUS WHERE Variable_name = 'Threads_connected'";
            #[derive(Debug, FromQueryResult)]
            struct Row {
                value: i64,
            }
            Row::find_by_statement(Statement::from_sql_and_values(backend, sql, []))
                .one(&state.db)
                .await
                .ok()
                .flatten()
                .map(|r| r.value)
        }
        sea_orm::DatabaseBackend::Postgres => {
            let sql =
                "SELECT count(*)::bigint AS value FROM pg_stat_activity WHERE state = 'active'";
            #[derive(Debug, FromQueryResult)]
            struct Row {
                value: i64,
            }
            Row::find_by_statement(Statement::from_sql_and_values(backend, sql, []))
                .one(&state.db)
                .await
                .ok()
                .flatten()
                .map(|r| r.value)
        }
        _ => None,
    };

    Ok(Json(ApiResponse::success(serde_json::json!({
        "status": if ping_ok { "connected" } else { "disconnected" },
        "active_connections": active_connections,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))))
}
