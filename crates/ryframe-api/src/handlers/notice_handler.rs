use serde_json;
use axum::{extract::{Path, Query, State}, routing::get, Json, Router};
use ryframe_common::AppResult;
use ryframe_core::PageQuery;
use ryframe_service::system::NoticeVo;
use validator::Validate;
use crate::dto::notice_dto::{CreateNoticeDto, UpdateNoticeDto};

use super::auth_handler::AppState;

pub fn notice_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", get(detail).put(update).delete(remove))
        .with_state(state)
}

async fn list(State(state): State<AppState>, Query(query): Query<PageQuery>) -> AppResult<Json<ryframe_core::PageResult<NoticeVo>>> {
    state.notice_service.find_by_page(&state.db, query).await.map(Json)
}

async fn detail(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<NoticeVo>> {
    match state.notice_service.find_by_id(&state.db, id).await? {
        Some(notice) => Ok(Json(notice)),
        None => Err(ryframe_common::AppError::NotFound("通知公告不存在".into())),
    }
}

async fn create(State(state): State<AppState>, Json(dto): Json<CreateNoticeDto>) -> AppResult<Json<NoticeVo>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.notice_service.create(&state.db, &dto.title, &dto.content, dto.notice_type.as_deref(), None).await.map(Json)
}

async fn update(State(state): State<AppState>, Path(id): Path<i64>, Json(dto): Json<UpdateNoticeDto>) -> AppResult<Json<NoticeVo>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.notice_service.update(&state.db, id, &dto.title, &dto.content, dto.notice_type.as_deref(), dto.status).await.map(Json)
}

async fn remove(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<serde_json::Value>> {
    state.notice_service.delete(&state.db, id).await?;
    Ok(Json(serde_json::json!({"message": "删除成功"})))
}
