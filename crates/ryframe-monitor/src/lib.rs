mod cache_monitor;
mod server_info;

use axum::{Json, extract::State};
use ryframe_common::AppResult;
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
pub fn monitor_router(state: MonitorState) -> axum::Router {
    axum::Router::new()
        .route("/server", axum::routing::get(server_info_handler))
        .route("/health", axum::routing::get(health_check_handler))
        .route("/cache", axum::routing::get(cache_info_handler))
        .route(
            "/cache/commands",
            axum::routing::get(cache_commands_handler),
        )
        .route("/db-pool", axum::routing::get(db_pool_handler))
        .with_state(state)
}

/// 服务器信息
async fn server_info_handler() -> AppResult<Json<ServerInfo>> {
    Ok(Json(ServerInfo::collect()))
}

/// 增强健康检查（含 DB + Redis 连通性）
async fn health_check_handler(
    State(state): State<MonitorState>,
) -> AppResult<Json<serde_json::Value>> {
    let db_ok = ryframe_db::connection::ping(&state.db).await.is_ok();
    let redis_ok = match state.redis.as_ref() {
        Some(r) => r.ping().await.is_ok(),
        None => false,
    };
    let redis_configured = state.redis.is_some();
    let all_ok = db_ok && (!redis_configured || redis_ok);

    Ok(Json(serde_json::json!({
        "status": if all_ok { "UP" } else { "DOWN" },
        "database": if db_ok { "connected" } else { "disconnected" },
        "redis": if redis_configured {
            if redis_ok { "connected" } else { "disconnected" }
        } else {
            "not_configured"
        },
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })))
}

/// 缓存信息
async fn cache_info_handler(
    State(state): State<MonitorState>,
) -> AppResult<Json<cache_monitor::CacheInfo>> {
    let info = cache_monitor::get_cache_info(state.redis.as_ref()).await;
    Ok(Json(info))
}

/// 缓存命令统计
async fn cache_commands_handler(
    State(state): State<MonitorState>,
) -> AppResult<Json<serde_json::Value>> {
    match state.redis.as_ref() {
        Some(redis) => {
            let stats = cache_monitor::get_cache_command_stats(redis).await;
            Ok(Json(stats.unwrap_or(
                serde_json::json!({"error": "无法获取命令统计"}),
            )))
        }
        None => Ok(Json(serde_json::json!({"error": "Redis 未配置"}))),
    }
}

/// 数据库连接池状态
async fn db_pool_handler(State(state): State<MonitorState>) -> AppResult<Json<serde_json::Value>> {
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

    Ok(Json(serde_json::json!({
        "status": if ping_ok { "connected" } else { "disconnected" },
        "active_connections": active_connections,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })))
}
