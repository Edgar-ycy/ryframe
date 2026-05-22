
mod server_info;

use axum::{extract::State, Json};
use ryframe_common::AppResult;
use sea_orm::DatabaseConnection;

pub use server_info::ServerInfo;

/// 监控路由状态
#[derive(Clone)]
pub struct MonitorState {
    pub db: DatabaseConnection,
}

/// 监控路由
pub fn monitor_router(state: MonitorState) -> axum::Router {
    axum::Router::new()
        .route("/server", axum::routing::get(server_info_handler))
        .route("/health", axum::routing::get(health_check_handler))
        .with_state(state)
}

/// 服务器信息
async fn server_info_handler() -> AppResult<Json<ServerInfo>> {
    Ok(Json(ServerInfo::collect()))
}

/// 增强健康检查（含 DB 连通性）
async fn health_check_handler(
    State(state): State<MonitorState>,
) -> AppResult<Json<serde_json::Value>> {
    let db_ok = ryframe_db::connection::ping(&state.db).await.is_ok();
    Ok(Json(serde_json::json!({
        "status": if db_ok { "UP" } else { "DOWN" },
        "database": if db_ok { "connected" } else { "disconnected" },
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })))
}