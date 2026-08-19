use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use ryframe_auth::RequestPrincipal;
use ryframe_http::{ApiPageResponse, ApiResponse, HttpResult};
use ryframe_kernel::AppError;
use ryframe_macro::{delete, get, post, put, route};

use crate::{
    dto::{
        public_dto::{JobScheduleExecutionVo, JobSchedulePreview, JobScheduleVo, ScheduleTargetVo},
        schedule_dto::{
            CreateScheduleRequest, ScheduleExecutionPageQuery, SchedulePageQuery,
            SchedulePreviewRequest, ScheduleVersionRequest, UpdateScheduleRequest,
            UpdateScheduleStatusRequest,
        },
    },
    state::AppState,
};

/// 定时任务管理路由。
pub fn schedule_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(targets))
        .merge(route!(preview))
        .merge(route!(list))
        .merge(route!(detail))
        .merge(route!(create))
        .merge(route!(update))
        .merge(route!(update_status))
        .merge(route!(run_now))
        .merge(route!(remove))
        .merge(route!(executions))
        .with_state(state)
}

#[get("/schedules/targets")]
#[perm("monitor:schedule:list")]
#[utoipa::path(
    get,
    path = "/api/v1/monitor/schedules/targets",
    operation_id = "listScheduleTargets",
    tag = "定时任务",
    responses((status = 200, description = "当前租户可见的调度目标", body = ApiResponse<Vec<ScheduleTargetVo>>)),
    security(("bearer" = []))
)]
async fn targets(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> HttpResult<Json<ApiResponse<Vec<ScheduleTargetVo>>>> {
    schedule_service(&state)?
        .targets_for_tenant(&current_user)
        .map_err(ryframe_http::HttpAppError::from)
        .map(|targets| targets.into_iter().map(ScheduleTargetVo::from).collect())
        .map(ApiResponse::success)
        .map(Json)
}

#[post("/schedules/preview")]
#[perm("monitor:schedule:list")]
#[utoipa::path(
    post,
    path = "/api/v1/monitor/schedules/preview",
    operation_id = "previewJobSchedule",
    tag = "定时任务",
    request_body = SchedulePreviewRequest,
    responses(
        (status = 200, description = "未来五次执行时间", body = ApiResponse<JobSchedulePreview>),
        (status = 400, description = "Cron 表达式或时区无效")
    ),
    security(("bearer" = []))
)]
async fn preview(
    State(state): State<AppState>,
    _current_user: RequestPrincipal,
    Json(request): Json<SchedulePreviewRequest>,
) -> HttpResult<Json<ApiResponse<JobSchedulePreview>>> {
    schedule_service(&state)?
        .preview(&request.cron_expression, &request.timezone)
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(JobSchedulePreview::from)
        .map(ApiResponse::success)
        .map(Json)
}

#[get("/schedules")]
#[perm("monitor:schedule:list")]
#[utoipa::path(
    get,
    path = "/api/v1/monitor/schedules",
    operation_id = "listJobSchedules",
    tag = "定时任务",
    params(SchedulePageQuery),
    responses((status = 200, description = "定时任务列表", body = ApiPageResponse<JobScheduleVo>)),
    security(("bearer" = []))
)]
async fn list(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<SchedulePageQuery>,
) -> HttpResult<Json<ApiPageResponse<JobScheduleVo>>> {
    let page = schedule_service(&state)?
        .list(
            &current_user,
            query.into_service_params(&state.config.pagination)?,
        )
        .await?;
    Ok(Json(ApiPageResponse::page(
        page.records.into_iter().map(JobScheduleVo::from).collect(),
        page.total,
        page.page,
        page.page_size,
        state.config.pagination.max_page_size,
    )))
}

#[get("/schedules/{id}")]
#[perm("monitor:schedule:list")]
#[utoipa::path(
    get,
    path = "/api/v1/monitor/schedules/{id}",
    operation_id = "getJobSchedule",
    tag = "定时任务",
    params(("id" = String, Path, description = "定时任务 ID")),
    responses(
        (status = 200, description = "定时任务详情", body = ApiResponse<JobScheduleVo>),
        (status = 404, description = "记录不可见或不存在")
    ),
    security(("bearer" = []))
)]
async fn detail(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
) -> HttpResult<Json<ApiResponse<JobScheduleVo>>> {
    schedule_service(&state)?
        .get(&current_user, parse_schedule_id(&id)?)
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(JobScheduleVo::from)
        .map(ApiResponse::success)
        .map(Json)
}

#[post("/schedules")]
#[perm("monitor:schedule:add")]
#[utoipa::path(
    post,
    path = "/api/v1/monitor/schedules",
    operation_id = "createJobSchedule",
    tag = "定时任务",
    request_body = CreateScheduleRequest,
    responses(
        (status = 200, description = "定时任务已创建", body = ApiResponse<JobScheduleVo>),
        (status = 400, description = "输入无效"),
        (status = 403, description = "目标范围越权"),
        (status = 409, description = "启用数量超过租户限制")
    ),
    security(("bearer" = []))
)]
async fn create(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(request): Json<CreateScheduleRequest>,
) -> HttpResult<Json<ApiResponse<JobScheduleVo>>> {
    schedule_service(&state)?
        .create(&current_user, request.into())
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(JobScheduleVo::from)
        .map(ApiResponse::success)
        .map(Json)
}

#[put("/schedules/{id}")]
#[perm("monitor:schedule:edit")]
#[utoipa::path(
    put,
    path = "/api/v1/monitor/schedules/{id}",
    operation_id = "updateJobSchedule",
    tag = "定时任务",
    params(("id" = String, Path, description = "定时任务 ID")),
    request_body = UpdateScheduleRequest,
    responses(
        (status = 200, description = "定时任务已更新", body = ApiResponse<JobScheduleVo>),
        (status = 404, description = "记录不可见或不存在"),
        (status = 409, description = "版本冲突或启用数量超限")
    ),
    security(("bearer" = []))
)]
async fn update(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
    Json(request): Json<UpdateScheduleRequest>,
) -> HttpResult<Json<ApiResponse<JobScheduleVo>>> {
    schedule_service(&state)?
        .update(&current_user, parse_schedule_id(&id)?, request.into())
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(JobScheduleVo::from)
        .map(ApiResponse::success)
        .map(Json)
}

#[put("/schedules/{id}/status")]
#[perm("monitor:schedule:edit")]
#[utoipa::path(
    put,
    path = "/api/v1/monitor/schedules/{id}/status",
    operation_id = "updateJobScheduleStatus",
    tag = "定时任务",
    params(("id" = String, Path, description = "定时任务 ID")),
    request_body = UpdateScheduleStatusRequest,
    responses(
        (status = 200, description = "启停状态已更新", body = ApiResponse<JobScheduleVo>),
        (status = 409, description = "版本冲突或启用数量超限")
    ),
    security(("bearer" = []))
)]
async fn update_status(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
    Json(request): Json<UpdateScheduleStatusRequest>,
) -> HttpResult<Json<ApiResponse<JobScheduleVo>>> {
    schedule_service(&state)?
        .set_enabled(
            &current_user,
            parse_schedule_id(&id)?,
            request.version,
            request.enabled,
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(JobScheduleVo::from)
        .map(ApiResponse::success)
        .map(Json)
}

#[post("/schedules/{id}/run")]
#[perm("monitor:schedule:run")]
#[utoipa::path(
    post,
    path = "/api/v1/monitor/schedules/{id}/run",
    operation_id = "runJobScheduleNow",
    tag = "定时任务",
    params(
        ("id" = String, Path, description = "定时任务 ID"),
        ("Idempotency-Key" = String, Header, description = "立即执行幂等键")
    ),
    responses(
        (status = 202, description = "任务已入队", body = ApiResponse<JobScheduleExecutionVo>),
        (status = 409, description = "禁止并发冲突")
    ),
    security(("bearer" = []))
)]
async fn run_now(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> HttpResult<(StatusCode, Json<ApiResponse<JobScheduleExecutionVo>>)> {
    let key = headers
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::Validation("缺少有效的 Idempotency-Key 请求头".into()))?;
    let execution = schedule_service(&state)?
        .run_now(&current_user, parse_schedule_id(&id)?, key)
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success(execution.into())),
    ))
}

#[delete("/schedules/{id}")]
#[perm("monitor:schedule:remove")]
#[utoipa::path(
    delete,
    path = "/api/v1/monitor/schedules/{id}",
    operation_id = "deleteJobSchedule",
    tag = "定时任务",
    params(("id" = String, Path, description = "定时任务 ID")),
    request_body = ScheduleVersionRequest,
    responses(
        (status = 200, description = "定时任务已软删除", body = ryframe_http::ApiEmptyResponse),
        (status = 409, description = "版本冲突")
    ),
    security(("bearer" = []))
)]
async fn remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
    Json(request): Json<ScheduleVersionRequest>,
) -> HttpResult<Json<ApiResponse<()>>> {
    schedule_service(&state)?
        .remove(&current_user, parse_schedule_id(&id)?, request.version)
        .await?;
    Ok(Json(ApiResponse::success_no_data()))
}

#[get("/schedules/{id}/executions")]
#[perm("monitor:schedule:list")]
#[utoipa::path(
    get,
    path = "/api/v1/monitor/schedules/{id}/executions",
    operation_id = "listJobScheduleExecutions",
    tag = "定时任务",
    params(
        ("id" = String, Path, description = "定时任务 ID"),
        ScheduleExecutionPageQuery
    ),
    responses((status = 200, description = "计划执行历史", body = ApiPageResponse<JobScheduleExecutionVo>)),
    security(("bearer" = []))
)]
async fn executions(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
    Query(query): Query<ScheduleExecutionPageQuery>,
) -> HttpResult<Json<ApiPageResponse<JobScheduleExecutionVo>>> {
    let page = schedule_service(&state)?
        .executions(
            &current_user,
            parse_schedule_id(&id)?,
            query.into_service_params(&state.config.pagination)?,
        )
        .await?;
    Ok(Json(ApiPageResponse::page(
        page.records
            .into_iter()
            .map(JobScheduleExecutionVo::from)
            .collect(),
        page.total,
        page.page,
        page.page_size,
        state.config.pagination.max_page_size,
    )))
}

fn parse_schedule_id(value: &str) -> HttpResult<i64> {
    Ok(value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| AppError::Validation("定时任务 ID 必须是正整数".into()))?)
}

fn schedule_service(state: &AppState) -> HttpResult<&ryframe_application::JobScheduleService> {
    state.services.job_schedules.as_deref().ok_or_else(|| {
        ryframe_http::HttpAppError::from(AppError::NotFound("定时任务调度未启用".into()))
    })
}
