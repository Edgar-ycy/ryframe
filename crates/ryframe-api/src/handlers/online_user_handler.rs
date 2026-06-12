use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get},
};
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_service::system::online_user_service::OnlineUserVo;

use crate::handlers::auth_handler::AppState;
use crate::list_query;

list_query!(pub OnlineUserQuery {
    username: String,
    ipaddr: String,
});

/// 在线用户路由
pub fn online_user_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list_online_users))
        .route("/list", get(list_online_users_page))
        .route("/listNoPage", get(list_online_users))
        .route("/{token_id}", delete(force_logout))
        .with_state(state)
}

/// 获取在线用户列表
/// 获取在线用户列表
#[utoipa::path(get, path = "/api/v1/system/online", tag = "在线用户",
    responses((status = 200, description = "在线用户列表")), security(("bearer" = [])))]
pub async fn list_online_users(
    State(state): State<AppState>,
    Query(query): Query<OnlineUserQuery>,
) -> AppResult<Json<ApiResponse<Vec<OnlineUserVo>>>> {
    let filtered = state
        .online_user_service
        .list_filtered(query.username.as_deref(), query.ipaddr.as_deref())
        .await;

    Ok(Json(ApiResponse::success(filtered)))
}

/// 获取在线用户列表（分页）
#[utoipa::path(get, path = "/api/v1/system/online/list", tag = "在线用户",
    responses((status = 200, description = "在线用户列表")),
    security(("bearer" = [])))]
pub async fn list_online_users_page(
    State(state): State<AppState>,
    Query(query): Query<OnlineUserQuery>,
) -> AppResult<Json<ApiPageResponse<OnlineUserVo>>> {
    let (rows, total) = state
        .online_user_service
        .list_filtered_page(
            query.username.as_deref(),
            query.ipaddr.as_deref(),
            query.page,
            query.page_size,
        )
        .await;
    Ok(Json(ApiPageResponse::new(rows, total, "查询成功")))
}

/// 强制下线用户
/// 强制下线用户
#[utoipa::path(delete, path = "/api/v1/system/online/{token_id}", tag = "在线用户",
    params(("token_id" = String, Path)), responses((status = 200, description = "强退成功")), security(("bearer" = [])))]
pub async fn force_logout(
    State(state): State<AppState>,
    Path(token_id): Path<String>,
) -> AppResult<Json<ApiResponse<()>>> {
    // 强制下线并获取 user_id
    let user_id = state.online_user_service.force_logout(&token_id).await?;

    // 黑名单 access token 的 jti
    let ttl = ryframe_auth::jwt::parse_duration(&state.config.auth.access_token_expire)
        .unwrap_or(3600) as u64;
    state.token_blacklist.blacklist(&token_id, ttl).await;

    // 也黑名单 user 级别 key（阻止通过 refresh_token 绕过强退）
    if user_id > 0 {
        let user_key = format!("force_logout:user:{}", user_id);
        state.token_blacklist.blacklist(&user_key, ttl).await;
    }

    Ok(Json(ApiResponse::success_no_data_with_msg(
        "用户已强制下线",
    )))
}
