use serde_json;
use axum::{extract::{Path, State}, routing::{get, post, put}, Json, Router};
use ryframe_common::AppResult;
use ryframe_db::repositories::dept_repo::DeptTreeNode;
use validator::Validate;
use crate::dto::dept_dto::{CreateDeptDto, UpdateDeptDto};

use super::auth_handler::AppState;

pub fn dept_router(state: AppState) -> Router {
    Router::new()
        .route("/tree", get(tree))
        .route("/", post(create))
        .route("/{id}", put(update).delete(remove))
        .with_state(state)
}

async fn tree(State(state): State<AppState>) -> AppResult<Json<Vec<DeptTreeNode>>> {
    state.dept_service.find_tree(&state.db).await.map(Json)
}

async fn create(
    State(state): State<AppState>,
    Json(dto): Json<CreateDeptDto>,
) -> AppResult<Json<ryframe_db::entities::dept::Model>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.dept_service.create(&state.db, &dto.name, dto.parent_id, dto.sort.unwrap_or(0)).await.map(Json)
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateDeptDto>,
) -> AppResult<Json<ryframe_db::entities::dept::Model>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.dept_service.update(&state.db, id, &dto.name, dto.parent_id, dto.sort.unwrap_or(0), dto.status).await.map(Json)
}

async fn remove(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<serde_json::Value>> {
    state.dept_service.delete(&state.db, id).await?;
    Ok(Json(serde_json::json!({"message": "删除成功"})))
}
