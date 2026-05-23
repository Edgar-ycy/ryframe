use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post, put},
};
use ryframe_common::AppResult;
use ryframe_core::Repository;
use ryframe_core::repository::PageQuery;
use ryframe_service::system::{JobLogVo, JobVo};
use serde::Deserialize;
use validator::Validate;

use crate::dto::job_dto::{CreateJobDto, UpdateJobDto};
use crate::handlers::auth_handler::AppState;

/// 任务日志分页查询参数
#[derive(Debug, Deserialize)]
pub struct JobLogPageQuery {
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_page_size")]
    pub page_size: u64,
    pub job_name: Option<String>,
    pub status: Option<String>,
}

fn default_page_size() -> u64 {
    10
}

pub fn job_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list).post(create_job))
        .route("/{id}", put(update).delete(remove))
        .route("/{id}/pause", post(pause_job))
        .route("/{id}/resume", post(resume_job))
        .route("/{id}/trigger", post(trigger))
        .route("/logs", get(log_list))
        .with_state(state)
}

/// 新建任务
/// 创建定时任务
#[utoipa::path(post, path = "/api/v1/system/jobs", tag = "定时任务",
    request_body = CreateJobDto, responses((status = 200, description = "创建成功")), security(("bearer" = [])))]
async fn create_job(
    State(state): State<AppState>,
    Json(dto): Json<CreateJobDto>,
) -> AppResult<Json<JobVo>> {
    dto.validate()
        .map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
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
    Ok(Json(job))
}

/// 删除任务
async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    state.job_service.delete(&state.db, id).await?;
    Ok(Json(serde_json::json!({"message": "删除成功"})))
}

/// 列出全部任务
/// 任务列表
#[utoipa::path(get, path = "/api/v1/system/jobs", tag = "定时任务",
    responses((status = 200, description = "任务列表")), security(("bearer" = [])))]
async fn list(State(state): State<AppState>) -> AppResult<Json<Vec<JobVo>>> {
    let jobs = state.job_service.list_all(&state.db).await?;
    Ok(Json(jobs))
}

/// 更新任务
async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateJobDto>,
) -> AppResult<Json<serde_json::Value>> {
    state
        .job_service
        .update(&state.db, id, dto.cron_expr, dto.status, dto.remark)
        .await?;
    Ok(Json(serde_json::json!({"message": "更新成功"})))
}

/// 暂停任务
/// 暂停任务
#[utoipa::path(post, path = "/api/v1/system/jobs/{id}/pause", tag = "定时任务",
    params(("id" = i64, Path)), responses((status = 200, description = "暂停成功")), security(("bearer" = [])))]
async fn pause_job(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    state.job_service.pause(&state.db, id).await?;
    Ok(Json(serde_json::json!({"message": "暂停成功"})))
}

/// 恢复任务
/// 恢复任务
#[utoipa::path(post, path = "/api/v1/system/jobs/{id}/resume", tag = "定时任务",
    params(("id" = i64, Path)), responses((status = 200, description = "恢复成功")), security(("bearer" = [])))]
async fn resume_job(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    state.job_service.resume(&state.db, id).await?;
    Ok(Json(serde_json::json!({"message": "恢复成功"})))
}

/// 立即触发一次
async fn trigger(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ryframe_task::TaskHistory>> {
    let entity = state
        .job_service
        .job_repo
        .find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ryframe_common::AppError::NotFound("任务不存在".into()))?;

    let history = state.job_service.trigger_once(&entity.name).await?;
    Ok(Json(history))
}

/// 执行历史分页
async fn log_list(
    State(state): State<AppState>,
    Query(q): Query<JobLogPageQuery>,
) -> AppResult<Json<ryframe_core::repository::PageResult<JobLogVo>>> {
    let query = PageQuery {
        page: q.page,
        page_size: q.page_size,
    };
    let result = state
        .job_service
        .log_page(&state.db, query, q.job_name.as_deref(), q.status)
        .await?;
    Ok(Json(result))
}
