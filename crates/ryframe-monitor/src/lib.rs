mod cache_monitor;
pub mod server_info;

use axum::{Json, extract::State};
use ryframe_auth::middleware::AuthState;
use ryframe_common::{ApiResponse, AppResult};
use ryframe_core::RedisClient;
use ryframe_macro::{get, route};
use sea_orm::{DatabaseBackend, DatabaseConnection, Statement};
pub use server_info::ServerInfo;

#[derive(Debug, sea_orm::FromQueryResult)]
struct ActiveConnectionRow {
    value: i64,
}

/// Monitor route state.
#[derive(Clone)]
pub struct MonitorState {
    pub db: DatabaseConnection,
    pub redis: Option<RedisClient>,
}

/// Build monitor routes.
///
/// When `auth_state` is `Some`, sensitive routes (`/server`, `/cache`,
/// `/cache/commands`, `/db-pool`) require authentication and permissions.
/// `/health` and `/metrics` are always public.
pub fn monitor_router(state: MonitorState, auth_state: Option<AuthState>) -> axum::Router {
    use axum::{middleware, routing::get as axum_get};

    let public = axum::Router::new()
        .route("/health", axum_get(health_check_handler))
        .route("/metrics", axum_get(metrics_handler));

    let mut protected = axum::Router::new()
        .merge(route!(server_info_handler))
        .merge(route!(cache_info_handler))
        .merge(route!(cache_commands_handler))
        .merge(route!(db_pool_handler));

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

#[get("/server")]
#[perm("monitor:server:list")]
async fn server_info_handler(
    State(_state): State<MonitorState>,
) -> AppResult<Json<ApiResponse<ServerInfo>>> {
    Ok(Json(ApiResponse::success(ServerInfo::collect())))
}

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
        "timestamp": current_timestamp(),
    }))))
}

#[get("/cache")]
#[perm("monitor:cache:list")]
async fn cache_info_handler(
    State(state): State<MonitorState>,
) -> AppResult<Json<ApiResponse<cache_monitor::CacheInfo>>> {
    let info = cache_monitor::get_cache_info(state.redis.as_ref()).await;
    Ok(Json(ApiResponse::success(info)))
}

#[get("/cache/commands")]
#[perm("monitor:cache:list")]
async fn cache_commands_handler(
    State(state): State<MonitorState>,
) -> AppResult<Json<ApiResponse<serde_json::Value>>> {
    match state.redis.as_ref() {
        Some(redis) => {
            let stats = cache_monitor::get_cache_command_stats(redis).await;
            Ok(Json(ApiResponse::success(stats.unwrap_or(
                serde_json::json!({"error": "failed to fetch command stats"}),
            ))))
        }
        None => Ok(Json(ApiResponse::success(
            serde_json::json!({"error": "Redis not configured"}),
        ))),
    }
}

async fn metrics_handler() -> axum::response::Response {
    let text = ryframe_middleware::metrics::metrics_text();
    text_response(text, "text/plain; version=0.0.4")
}

#[get("/db-pool")]
#[perm("monitor:db-pool:list")]
async fn db_pool_handler(
    State(state): State<MonitorState>,
) -> AppResult<Json<ApiResponse<serde_json::Value>>> {
    let backend = state.db.get_database_backend();
    let ping_ok = ryframe_db::connection::ping(&state.db).await.is_ok();

    let active_connections = match backend {
        DatabaseBackend::MySql => {
            let sql = "SHOW STATUS WHERE Variable_name = 'Threads_connected'";
            active_connection_count(&state.db, backend, sql).await
        }
        DatabaseBackend::Postgres => {
            let sql =
                "SELECT count(*)::bigint AS value FROM pg_stat_activity WHERE state = 'active'";
            active_connection_count(&state.db, backend, sql).await
        }
        _ => None,
    };

    Ok(Json(ApiResponse::success(serde_json::json!({
        "status": if ping_ok { "connected" } else { "disconnected" },
        "active_connections": active_connections,
        "timestamp": current_timestamp(),
    }))))
}

fn current_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn text_response(text: String, content_type: &str) -> axum::response::Response {
    axum::response::Response::builder()
        .header("Content-Type", content_type)
        .body(axum::body::Body::from(text))
        .unwrap()
}

async fn active_connection_count(
    db: &DatabaseConnection,
    backend: DatabaseBackend,
    sql: &str,
) -> Option<i64> {
    <ActiveConnectionRow as sea_orm::FromQueryResult>::find_by_statement(
        Statement::from_sql_and_values(backend, sql, []),
    )
    .one(db)
    .await
    .ok()
    .flatten()
    .map(|row| row.value)
}
