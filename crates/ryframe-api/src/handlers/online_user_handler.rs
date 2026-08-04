use crate::dto::public_dto::OnlineUserVo;
use crate::list_query;
use crate::state::AppState;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use ryframe_auth::RequestPrincipal;
use ryframe_http::{ApiPageResponse, ApiResponse, HttpResult};
use ryframe_kernel::AppError;
use ryframe_macro::{delete, get, route};

list_query!(pub OnlineUserQuery, OnlineUserFilterQuery {
    username: String,
    ipaddr: String,
});

/// 在线用户路由
pub fn online_user_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list_online_users_page))
        .merge(route!(force_logout))
        .with_state(state)
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
) -> HttpResult<Json<ApiPageResponse<OnlineUserVo>>> {
    let (page, filter) = query.into_parts(&state.config.pagination)?;
    let response_page = page.page();
    let response_page_size = page.page_size();
    let (rows, total) = state
        .services
        .online_user
        .list_filtered_page(
            &current_user,
            filter.username.as_deref(),
            filter.ipaddr.as_deref(),
            page,
        )
        .await?;
    Ok(Json(ApiPageResponse::page(
        rows.into_iter().map(OnlineUserVo::from).collect(),
        total,
        response_page,
        response_page_size,
        state.config.pagination.max_page_size,
    )))
}

/// 强制下线用户
#[delete("/{sid}")]
#[perm("monitor:online:force-logout")]
/// 强制下线用户
#[utoipa::path(delete, path = "/api/v1/system/online/{sid}", tag = "在线用户",
    params(("sid" = String, Path, description = "稳定的设备会话标识")),
    responses(
        (status = 200, description = "强退成功", body = ryframe_http::ApiEmptyResponse),
        (status = 404, description = "会话不存在或不属于当前租户"),
        (status = 503, description = "Redis 会话服务不可用")
    ),
    security(("bearer" = [])))]
pub async fn force_logout(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(sid): Path<String>,
) -> HttpResult<Json<ApiResponse<()>>> {
    // 刷新令牌族是权威状态。先撤销它，同时原子校验 tenant + sid。若 Redis
    // 失败则返回 503 而不删除展示索引，同一请求可以安全重试。
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
        return Err(AppError::NotFound("在线会话不存在".into()).into());
    }

    // 这是尽力而为的二级索引清理。已撤销的令牌族会使该 sid 的所有访问/刷新
    // 令牌均无法使用。
    state
        .services
        .online_user
        .remove_user(&current_user.tenant_id, &sid)
        .await;

    Ok(Json(ApiResponse::success_no_data()))
}
