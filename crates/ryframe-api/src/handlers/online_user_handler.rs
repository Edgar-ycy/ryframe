use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use ryframe_auth::RequestPrincipal;
use ryframe_common::{ApiPageResponse, ApiResponse, AppError, AppResult};
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
#[delete("/{sid}")]
#[perm("monitor:online:force-logout")]
/// 强制下线用户
#[utoipa::path(delete, path = "/api/v1/system/online/{sid}", tag = "在线用户",
    params(("sid" = String, Path, description = "Stable device-session identifier")),
    responses(
        (status = 200, description = "强退成功", body = ryframe_common::ApiEmptyResponse),
        (status = 404, description = "会话不存在或不属于当前租户"),
        (status = 503, description = "Redis 会话服务不可用")
    ),
    security(("bearer" = [])))]
pub async fn force_logout(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(sid): Path<String>,
) -> AppResult<Json<ApiResponse<()>>> {
    // The refresh family is authoritative. Revoke it first, atomically
    // validating tenant + sid. A Redis failure therefore returns 503 without
    // deleting the display index, and the same request can safely be retried.
    let revoked = state
        .services
        .auth
        .refresh_sessions()
        .revoke_for_tenant(&current_user.tenant_id, &sid)
        .await
        .inspect_err(|error| {
            if matches!(error, AppError::ServiceUnavailable(_)) {
                ryframe_middleware::metrics::record_redis_degraded("force_logout_session");
            }
        })?;
    if !revoked {
        return Err(AppError::NotFound("在线会话不存在".into()));
    }

    // This is a best-effort secondary index cleanup. The already-revoked
    // family makes every access/refresh token for the sid unusable.
    state
        .services
        .online_user
        .remove_user(&current_user.tenant_id, &sid)
        .await;

    Ok(Json(ApiResponse::success_no_data_with_msg(
        "用户已强制下线",
    )))
}
