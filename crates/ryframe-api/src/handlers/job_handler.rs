use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, post, put},
};
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_core::repository::PageQuery;
use ryframe_service::system::{JobLogVo, JobVo};
use serde::Deserialize;
use validator::Validate;

use crate::{
    dto::job_dto::{CreateJobDto, UpdateJobDto},
    handlers::auth_handler::AppState,
};

/// 任务日志分页查询参数
#[derive(Debug, Deserialize)]
pub struct JobLogPageQuery {
    #[serde(default)]
    pub page: u64,
    #[serde(
        default = "ryframe_core::repository::default_page_size",
        alias = "pageSize"
    )]
    pub page_size: u64,
    pub job_name: Option<String>,
    pub status: Option<String>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
}

/// 任务列表分页查询参数（带过滤）
#[derive(Debug, Deserialize)]
pub struct JobListQuery {
    #[serde(default)]
    pub page: u64,
    #[serde(
        default = "ryframe_core::repository::default_page_size",
        alias = "pageSize"
    )]
    pub page_size: u64,
    pub name: Option<String>,
    pub group_name: Option<String>,
    pub status: Option<String>,
}

pub fn job_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list_no_page))
        .route("/", post(create_job))
        .route("/list", get(list_page))
        .route("/listNoPage", get(list_no_page))
        .route("/{id}", put(update))
        .route("/{id}", delete(remove))
        .route("/{id}/pause", post(pause_job))
        .route("/{id}/resume", post(resume_job))
        .route("/{id}/trigger", post(trigger))
        .route("/logs", get(log_list))
        .route("/logs", axum::routing::delete(clear_logs))
        .with_state(state)
}

/// 新建任务
/// 创建定时任务
#[utoipa::path(post, path = "/api/v1/system/jobs", tag = "定时任务",
    request_body = CreateJobDto, responses((status = 200, description = "创建成功")), security(("bearer" = [])))]
async fn create_job(
    State(state): State<AppState>,
    Json(dto): Json<CreateJobDto>,
) -> AppResult<Json<ApiResponse<JobVo>>> {
    dto.validate()?;
    let job = state
        .job_service
        .create(
            &state.db,
            &dto.name,
            &dto.cron_expr,
            dto.group_name.as_deref(),
            dto.misfire_policy.as_deref(),
            dto.concurrent.as_deref(),
            dto.remark.as_deref(),
        )
        .await?;
    Ok(Json(ApiResponse::success(job)))
}

/// 删除任务
#[utoipa::path(delete, path = "/api/v1/system/jobs/{id}", tag = "定时任务",
    params(("id" = i64, Path)),
    responses((status = 200, description = "删除成功")),
    security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    state.job_service.delete(&state.db, id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("删除成功")))
}

/// 任务列表分页查询
#[utoipa::path(get, path = "/api/v1/system/jobs/list", tag = "定时任务",
    responses((status = 200, description = "任务列表")),
    security(("bearer" = [])))]
async fn list_page(
    State(state): State<AppState>,
    Query(q): Query<JobListQuery>,
) -> AppResult<Json<ApiPageResponse<JobVo>>> {
    let query = PageQuery {
        page: q.page,
        page_size: q.page_size,
    };
    let result = state
        .job_service
        .list_page(
            &state.db,
            query,
            q.name.as_deref(),
            q.group_name.as_deref(),
            q.status.as_deref(),
        )
        .await?;
    Ok(Json(result.to_page_response("查询成功")))
}

/// 列出全部任务（不分页）
/// 任务列表
#[utoipa::path(get, path = "/api/v1/system/jobs", tag = "定时任务",
    responses((status = 200, description = "任务列表")), security(("bearer" = [])))]
async fn list_no_page(State(state): State<AppState>) -> AppResult<Json<ApiResponse<Vec<JobVo>>>> {
    let jobs = state.job_service.list_all(&state.db).await?;
    Ok(Json(ApiResponse::success(jobs)))
}

/// 更新任务
#[utoipa::path(put, path = "/api/v1/system/jobs/{id}", tag = "定时任务",
    params(("id" = i64, Path)),
    request_body = UpdateJobDto,
    responses((status = 200, description = "更新成功")),
    security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateJobDto>,
) -> AppResult<Json<ApiResponse<()>>> {
    state
        .job_service
        .update(
            &state.db,
            id,
            dto.cron_expr,
            dto.status,
            dto.remark,
            dto.misfire_policy,
            dto.concurrent,
        )
        .await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("更新成功")))
}

/// 暂停任务
/// 暂停任务
#[utoipa::path(post, path = "/api/v1/system/jobs/{id}/pause", tag = "定时任务",
    params(("id" = i64, Path)), responses((status = 200, description = "暂停成功")), security(("bearer" = [])))]
async fn pause_job(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    state.job_service.pause(&state.db, id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("暂停成功")))
}

/// 恢复任务
/// 恢复任务
#[utoipa::path(post, path = "/api/v1/system/jobs/{id}/resume", tag = "定时任务",
    params(("id" = i64, Path)), responses((status = 200, description = "恢复成功")), security(("bearer" = [])))]
async fn resume_job(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    state.job_service.resume(&state.db, id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("恢复成功")))
}

/// 立即触发一次任务
#[utoipa::path(post, path = "/api/v1/system/jobs/{id}/trigger", tag = "定时任务",
    params(("id" = i64, Path)),
    responses((status = 200, description = "执行完成")),
    security(("bearer" = [])))]
async fn trigger(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<ryframe_task::TaskHistory>>> {
    let history = state.job_service.trigger_by_id(&state.db, id).await?;
    Ok(Json(ApiResponse::success(history)))
}

/// 清空所有任务执行日志
#[utoipa::path(delete, path = "/api/v1/system/jobs/logs", tag = "定时任务",
    responses((status = 200, description = "清空成功")),
    security(("bearer" = [])))]
async fn clear_logs(State(state): State<AppState>) -> AppResult<Json<ApiResponse<u64>>> {
    let count = state.job_service.clean_logs(&state.db).await?;
    Ok(Json(ApiResponse::success_msg(
        format!("已清空 {} 条日志", count),
        count,
    )))
}

/// 任务执行历史分页
#[utoipa::path(get, path = "/api/v1/system/jobs/logs", tag = "定时任务",
    responses((status = 200, description = "日志列表")),
    security(("bearer" = [])))]
async fn log_list(
    State(state): State<AppState>,
    Query(q): Query<JobLogPageQuery>,
) -> AppResult<Json<ApiPageResponse<JobLogVo>>> {
    let query = PageQuery {
        page: q.page,
        page_size: q.page_size,
    };
    let parse_time = |s: &Option<String>| -> Option<chrono::DateTime<chrono::Utc>> {
        s.as_deref()
            .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc))
    };
    let result = state
        .job_service
        .log_page(
            &state.db,
            query,
            q.job_name.as_deref(),
            q.status,
            parse_time(&q.begin_time),
            parse_time(&q.end_time),
        )
        .await?;
    Ok(Json(result.to_page_response("查询成功")))
}
