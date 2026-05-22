use axum::{
    extract::{Path, Query, State},
    routing::{delete, get},
    Json, Router,
};
use ryframe_common::AppResult;
use ryframe_service::system::online_user_service::OnlineUserVo;
use serde::Deserialize;

use crate::handlers::auth_handler::AppState;

/// 在线用户查询参数
#[derive(Debug, Deserialize)]
pub struct OnlineUserQuery {
    pub username: Option<String>,
    pub ipaddr: Option<String>,
}

/// 在线用户路由
pub fn online_user_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list_online_users))
        .route("/{token_id}", delete(force_logout))
        .with_state(state)
}

/// 获取在线用户列表
pub async fn list_online_users(
    State(state): State<AppState>,
    Query(query): Query<OnlineUserQuery>,
) -> AppResult<Json<Vec<OnlineUserVo>>> {
    let users = state.online_user_service.list_online_users().await;

    // 过滤
    let filtered = users
        .into_iter()
        .filter(|u| {
            if let Some(username) = &query.username
                && !u.username.contains(username) {
                    return false;
                }
            if let Some(ipaddr) = &query.ipaddr
                && !u.ipaddr.contains(ipaddr) {
                    return false;
                }
            true
        })
        .collect();

    Ok(Json(filtered))
}

/// 强制下线用户
pub async fn force_logout(
    State(state): State<AppState>,
    Path(token_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    state.online_user_service.force_logout(&token_id).await?;

    Ok(Json(serde_json::json!({
        "code": 200,
        "message": "用户已强制下线"
    })))
}
