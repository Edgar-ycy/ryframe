use serde::Deserialize;
use serde_json;
use axum::{extract::{Path, Query, State}, routing::get, Json, Router};
use ryframe_common::AppResult;
use ryframe_core::PageQuery;
use ryframe_service::system::NoticeVo;
use validator::Validate;
use crate::dto::notice_dto::{CreateNoticeDto, UpdateNoticeDto};

use super::auth_handler::AppState;

/// 通知公告列表查询参数（支持搜索过滤）
#[derive(Debug, Deserialize)]
pub struct NoticeListQuery {
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_page_size")]
    pub page_size: u64,
    pub title: Option<String>,
    pub notice_type: Option<String>,
    pub status: Option<String>,
}

fn default_page_size() -> u64 {
    10
}

pub fn notice_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", get(detail).put(update).delete(remove))
        .with_state(state)
}

/// 通知公告列表
#[utoipa::path(get, path = "/api/v1/system/notices", tag = "通知公告",
    responses((status = 200, description = "公告列表")), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    Query(query): Query<NoticeListQuery>,
) -> AppResult<Json<ryframe_core::PageResult<NoticeVo>>> {
    let page_query = PageQuery { page: query.page, page_size: query.page_size };
    let has_filter = query.title.is_some() || query.notice_type.is_some() || query.status.is_some();
    if has_filter {
        state.notice_service
            .find_by_page_filtered(
                &state.db, page_query,
                query.title.as_deref(), query.notice_type.as_deref(), query.status.as_deref(),
            )
            .await.map(Json)
    } else {
        state.notice_service.find_by_page(&state.db, page_query).await.map(Json)
    }
}

async fn detail(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<NoticeVo>> {
    match state.notice_service.find_by_id(&state.db, id).await? {
        Some(notice) => Ok(Json(notice)),
        None => Err(ryframe_common::AppError::NotFound("通知公告不存在".into())),
    }
}

/// 创建通知公告
#[utoipa::path(post, path = "/api/v1/system/notices", tag = "通知公告",
    request_body = CreateNoticeDto, responses((status = 200, description = "创建成功")), security(("bearer" = [])))]
async fn create(State(state): State<AppState>, Json(dto): Json<CreateNoticeDto>) -> AppResult<Json<NoticeVo>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.notice_service.create(&state.db, &dto.title, &dto.content, dto.notice_type.as_deref(), None).await.map(Json)
}

async fn update(State(state): State<AppState>, Path(id): Path<i64>, Json(dto): Json<UpdateNoticeDto>) -> AppResult<Json<NoticeVo>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.notice_service.update(&state.db, id, &dto.title, &dto.content, dto.notice_type.as_deref(), dto.status).await.map(Json)
}

/// 删除通知公告
#[utoipa::path(delete, path = "/api/v1/system/notices/{id}", tag = "通知公告",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功")), security(("bearer" = [])))]
async fn remove(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<serde_json::Value>> {
    state.notice_service.delete(&state.db, id).await?;
    Ok(Json(serde_json::json!({"message": "删除成功"})))
}
