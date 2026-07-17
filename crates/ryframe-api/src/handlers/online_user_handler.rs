use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use ryframe_auth::RequestPrincipal;
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_macro::{delete, get, route};
use ryframe_service::system::online_user_service::OnlineUserVo;

use crate::list_query;
use crate::state::AppState;

list_query!(pub OnlineUserQuery, OnlineUserFilterQuery {
    username: String,
    ipaddr: String,
});

/// 在线用户路由
pub fn online_user_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list_online_users))
        .merge(route!(list_online_users_page))
        .merge(route!(force_logout))
        .with_state(state)
}

/// 获取在线用户列表
#[get("/all")]
#[perm("monitor:online:list")]
/// 获取在线用户列表
#[utoipa::path(get, path = "/api/v1/system/online/all", tag = "在线用户",
    params(OnlineUserFilterQuery),
    responses((status = 200, description = "在线用户列表", body = ApiResponse<Vec<OnlineUserVo>>)), security(("bearer" = [])))]
pub async fn list_online_users(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<OnlineUserFilterQuery>,
) -> AppResult<Json<ApiResponse<Vec<OnlineUserVo>>>> {
    let filtered = state
        .services
        .online_user
        .list_filtered(
            &current_user,
            query.username.as_deref(),
            query.ipaddr.as_deref(),
        )
        .await?;

    Ok(Json(ApiResponse::success(filtered)))
}

/// 获取在线用户列表（分页）
#[get("/")]
#[perm("monitor:online:list")]
#[utoipa::path(get, path = "/api/v1/system/online", tag = "在线用户",
    params(OnlineUserQuery),
    responses((status = 200, description = "在线用户列表", body = ApiPageResponse<OnlineUserVo>)),
    security(("bearer" = [])))]
pub async fn list_online_users_page(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<OnlineUserQuery>,
) -> AppResult<Json<ApiPageResponse<OnlineUserVo>>> {
    let (page, filter) = query.into_parts();
    let (rows, total) = state
        .services
        .online_user
        .list_filtered_page(
            &current_user,
            filter.username.as_deref(),
            filter.ipaddr.as_deref(),
            page.page,
            page.page_size,
        )
        .await?;
    Ok(Json(ApiPageResponse::new(rows, total, "查询成功")))
}

/// 强制下线用户
#[delete("/{token_id}")]
#[perm("monitor:online:force-logout")]
/// 强制下线用户
#[utoipa::path(delete, path = "/api/v1/system/online/{token_id}", tag = "在线用户",
    params(("token_id" = String, Path)), responses((status = 200, description = "强退成功", body = ryframe_common::ApiEmptyResponse)), security(("bearer" = [])))]
pub async fn force_logout(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(token_id): Path<String>,
) -> AppResult<Json<ApiResponse<()>>> {
    let session = state
        .services
        .online_user
        .force_logout(&current_user, &token_id)
        .await?;

    // 黑名单 access token 的 jti
    let ttl = ryframe_auth::jwt::parse_duration(&state.config.auth.access_token_expire)
        .unwrap_or(3600) as u64;
    state.token_blacklist.blacklist(&token_id, ttl).await;

    // 也黑名单 user 级别 key（阻止通过 refresh_token 绕过强退）
    let user_key = format!(
        "force_logout:{}:user:{}",
        session.tenant_id, session.user_id
    );
    state.token_blacklist.blacklist(&user_key, ttl).await;

    Ok(Json(ApiResponse::success_no_data_with_msg(
        "用户已强制下线",
    )))
}
