use axum::{extract::State, routing::get, Json, Router};
use ryframe_common::AppResult;
use ryframe_service::system::PermissionTreeNode;

use super::auth_handler::AppState;

pub fn permission_router(state: AppState) -> Router {
    Router::new()
        .route("/tree", get(tree))
        .with_state(state)
}

async fn tree(
    State(state): State<AppState>,
) -> AppResult<Json<Vec<PermissionTreeNode>>> {
    let tree = state.permission_service.find_tree(&state.db, None).await?;
    Ok(Json(tree))
}
