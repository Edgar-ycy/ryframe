use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use ryframe_auth::RequestPrincipal;
use ryframe_http::{ApiPageResponse, ApiResponse, AppError, AppResult};
use ryframe_macro::{get, post, route};
use ryframe_service::{BackgroundJobQueueStats, BackgroundJobVo};

use crate::{dto::job_dto::BackgroundJobPageQuery, state::AppState};

/// 后台任务监控路由。
pub fn job_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list))
        .merge(route!(stats))
        .merge(route!(retry_dead))
        .with_state(state)
}

/// 分页查询当前租户的后台任务。
#[get("/jobs")]
#[perm("monitor:job:list")]
#[utoipa::path(get, path = "/api/v1/monitor/jobs", tag = "后台任务",
    params(BackgroundJobPageQuery),
    responses((status = 200, description = "后台任务列表", body = ApiPageResponse<BackgroundJobVo>)),
    security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<BackgroundJobPageQuery>,
) -> AppResult<Json<ApiPageResponse<BackgroundJobVo>>> {
    state
        .services
        .job_queue
        .list_for_tenant(
            &current_user,
            query.into_service_params(&state.config.pagination)?,
        )
        .await
        .map_err(ryframe_http::AppError::from)
        .map(|page| {
            Json(ApiPageResponse::new(
                page.records,
                page.total,
                page.page,
                page.page_size,
                state.config.pagination.max_page_size,
                "查询成功",
            ))
        })
}

/// 统计当前租户的后台任务队列状态。
#[get("/jobs/stats")]
#[perm("monitor:job:list")]
#[utoipa::path(get, path = "/api/v1/monitor/jobs/stats", tag = "后台任务",
    responses((status = 200, description = "后台任务队列统计", body = ApiResponse<BackgroundJobQueueStats>)),
    security(("bearer" = [])))]
async fn stats(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> AppResult<Json<ApiResponse<BackgroundJobQueueStats>>> {
    state
        .services
        .job_queue
        .stats_for_tenant(&current_user)
        .await
        .map_err(ryframe_http::AppError::from)
        .map(ApiResponse::success)
        .map(Json)
}

/// 人工重新投递一条死信任务。
#[post("/jobs/{id}/retry")]
#[perm("monitor:job:retry")]
#[utoipa::path(post, path = "/api/v1/monitor/jobs/{id}/retry", tag = "后台任务",
    params(("id" = String, Path, description = "后台任务 ID")),
    responses(
        (status = 200, description = "任务已重新投递", body = ApiResponse<BackgroundJobVo>),
        (status = 404, description = "任务不存在或不属于当前租户"),
        (status = 409, description = "任务不是死信状态或状态已变化")
    ),
    security(("bearer" = [])))]
async fn retry_dead(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
) -> AppResult<Json<ApiResponse<BackgroundJobVo>>> {
    state
        .services
        .job_queue
        .retry_dead_for_tenant(&current_user, parse_job_id(&id)?)
        .await
        .map_err(ryframe_http::AppError::from)
        .map(|job| ApiResponse::success_msg("任务已重新投递", job))
        .map(Json)
}

fn parse_job_id(value: &str) -> AppResult<i64> {
    value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| AppError::Validation("后台任务 ID 必须是正整数".into()))
}

#[cfg(test)]
mod tests {
    use super::parse_job_id;

    #[test]
    fn task_id_must_be_a_positive_i64() {
        assert_eq!(parse_job_id("42").unwrap(), 42);
        assert!(parse_job_id("0").is_err());
        assert!(parse_job_id("not-an-id").is_err());
    }
}
