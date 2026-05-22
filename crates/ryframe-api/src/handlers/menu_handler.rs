use serde_json;
use axum::{extract::{Path, State}, routing::{get, post, put}, Json, Router};
use ryframe_common::AppResult;
use ryframe_db::repositories::menu_repo::MenuTreeNode;
use validator::Validate;
use crate::dto::menu_dto::{CreateMenuDto, UpdateMenuDto};

use super::auth_handler::AppState;

pub fn menu_router(state: AppState) -> Router {
    Router::new()
        .route("/tree", get(tree))
        .route("/", post(create))
        .route("/{id}", put(update).delete(remove))
        .with_state(state)
}

async fn tree(State(state): State<AppState>) -> AppResult<Json<Vec<MenuTreeNode>>> {
    state.menu_service.find_tree(&state.db).await.map(Json)
}

async fn create(
    State(state): State<AppState>,
    Json(dto): Json<CreateMenuDto>,
) -> AppResult<Json<ryframe_db::entities::menu::Model>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.menu_service.create(
        &state.db, &dto.name, dto.parent_id,
        dto.path.as_deref(), dto.component.as_deref(), dto.icon.as_deref(),
        dto.sort.unwrap_or(0), dto.visible.unwrap_or(true),
    ).await.map(Json)
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateMenuDto>,
) -> AppResult<Json<ryframe_db::entities::menu::Model>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.menu_service.update(
        &state.db, id, &dto.name, dto.parent_id,
        dto.path.as_deref(), dto.component.as_deref(), dto.icon.as_deref(),
        dto.sort.unwrap_or(0), dto.visible.unwrap_or(true), dto.status,
    ).await.map(Json)
}

async fn remove(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<serde_json::Value>> {
    state.menu_service.delete(&state.db, id).await?;
    Ok(Json(serde_json::json!({"message": "删除成功"})))
}
