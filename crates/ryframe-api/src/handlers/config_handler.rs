use serde_json;
use axum::{extract::{Path, Query, State}, routing::get, Json, Router};
use ryframe_common::AppResult;
use ryframe_core::PageQuery;
use ryframe_service::system::ConfigVo;
use validator::Validate;
use crate::dto::config_dto::{CreateConfigDto, UpdateConfigDto};

use super::auth_handler::AppState;

pub fn config_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", get(detail).put(update).delete(remove))
        .with_state(state)
}

async fn list(State(state): State<AppState>, Query(query): Query<PageQuery>) -> AppResult<Json<ryframe_core::PageResult<ConfigVo>>> {
    state.config_service.find_by_page(&state.db, query).await.map(Json)
}

async fn detail(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<ConfigVo>> {
    match state.config_service.find_by_id(&state.db, id).await? {
        Some(cfg) => Ok(Json(cfg)),
        None => Err(ryframe_common::AppError::NotFound("参数配置不存在".into())),
    }
}

async fn create(
    State(state): State<AppState>,
    Json(dto): Json<CreateConfigDto>,
) -> AppResult<Json<ConfigVo>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.config_service.create(
        &state.db, &dto.name, &dto.key, &dto.value, dto.remark.as_deref(),
    ).await.map(Json)
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateConfigDto>,
) -> AppResult<Json<ConfigVo>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.config_service.update(&state.db, id, &dto.value).await.map(Json)
}

async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    state.config_service.delete(&state.db, id).await?;
    Ok(Json(serde_json::json!({"message": "删除成功"})))
}
