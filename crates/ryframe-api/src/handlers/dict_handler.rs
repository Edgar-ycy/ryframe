use serde::Deserialize;
use serde_json;
use axum::{extract::{Path, Query, State}, routing::{get, put}, Json, Router};
use ryframe_common::AppResult;
use ryframe_service::system::{DictDataVo, DictTypeVo};
use validator::Validate;
use crate::dto::dict_dto::{CreateDictDataDto, CreateDictTypeDto, UpdateDictDataDto, UpdateDictTypeDto};

use super::auth_handler::AppState;

pub fn dict_router(state: AppState) -> Router {
    Router::new()
        .route("/types", get(list_types).post(create_type))
        .route("/types/{id}", put(update_type).delete(delete_type))
        .route("/data", get(list_data).post(create_data))
        .route("/data/{id}", put(update_data).delete(delete_data))
        .with_state(state)
}

async fn list_types(State(state): State<AppState>) -> AppResult<Json<Vec<DictTypeVo>>> {
    state.dict_service.find_types(&state.db).await.map(Json)
}

async fn create_type(State(state): State<AppState>, Json(dto): Json<CreateDictTypeDto>) -> AppResult<Json<DictTypeVo>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.dict_service.create_type(&state.db, &dto.name, &dto.code).await.map(Json)
}

async fn update_type(State(state): State<AppState>, Path(id): Path<i64>, Json(dto): Json<UpdateDictTypeDto>) -> AppResult<Json<DictTypeVo>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.dict_service.update_type(&state.db, id, &dto.name, dto.status).await.map(Json)
}

async fn delete_type(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<serde_json::Value>> {
    state.dict_service.delete_type(&state.db, id).await?;
    Ok(Json(serde_json::json!({"message": "删除成功"})))
}

#[derive(Debug, Deserialize)]
struct ListDataQuery {
    type_code: String,
}

async fn list_data(State(state): State<AppState>, Query(query): Query<ListDataQuery>) -> AppResult<Json<Vec<DictDataVo>>> {
    state.dict_service.find_data_by_type(&state.db, &query.type_code).await.map(Json)
}

async fn create_data(State(state): State<AppState>, Json(dto): Json<CreateDictDataDto>) -> AppResult<Json<DictDataVo>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.dict_service.create_data(&state.db, &dto.type_code, &dto.label, &dto.value, dto.sort.unwrap_or(0)).await.map(Json)
}

async fn update_data(State(state): State<AppState>, Path(id): Path<i64>, Json(dto): Json<UpdateDictDataDto>) -> AppResult<Json<DictDataVo>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.dict_service.update_data(&state.db, id, &dto.label, &dto.value, dto.sort.unwrap_or(0), dto.status).await.map(Json)
}

async fn delete_data(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<serde_json::Value>> {
    state.dict_service.delete_data(&state.db, id).await?;
    Ok(Json(serde_json::json!({"message": "删除成功"})))
}
