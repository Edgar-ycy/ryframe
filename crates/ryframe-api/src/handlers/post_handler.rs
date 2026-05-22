use serde_json;
use axum::{extract::{Path, Query, State}, routing::get, Json, Router};
use ryframe_common::AppResult;
use ryframe_core::PageQuery;
use ryframe_service::system::PostVo;
use validator::Validate;
use crate::dto::post_dto::{CreatePostDto, UpdatePostDto};

use super::auth_handler::AppState;

pub fn post_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", get(detail).put(update).delete(remove))
        .with_state(state)
}

async fn list(State(state): State<AppState>, Query(query): Query<PageQuery>) -> AppResult<Json<ryframe_core::PageResult<PostVo>>> {
    state.post_service.find_by_page(&state.db, query).await.map(Json)
}

async fn detail(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<PostVo>> {
    match state.post_service.find_by_id(&state.db, id).await? {
        Some(post) => Ok(Json(post)),
        None => Err(ryframe_common::AppError::NotFound("岗位不存在".into())),
    }
}

async fn create(State(state): State<AppState>, Json(dto): Json<CreatePostDto>) -> AppResult<Json<PostVo>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.post_service.create(&state.db, &dto.name, &dto.code, dto.sort.unwrap_or(0)).await.map(Json)
}

async fn update(State(state): State<AppState>, Path(id): Path<i64>, Json(dto): Json<UpdatePostDto>) -> AppResult<Json<PostVo>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.post_service.update(&state.db, id, &dto.name, dto.sort.unwrap_or(0), dto.status).await.map(Json)
}

async fn remove(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<serde_json::Value>> {
    state.post_service.delete(&state.db, id).await?;
    Ok(Json(serde_json::json!({"message": "删除成功"})))
}
