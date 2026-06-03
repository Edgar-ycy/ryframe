use axum::{Json, Router, extract::State, routing::get};

use ryframe_common::{ApiResponse, AppResult};
use validator::Validate;

use crate::dto::CreateTodoDto;
use crate::model::Todo;
use crate::service::TodoService;

/// 应用共享状态
#[derive(Clone)]
pub struct AppState {
    pub todo_service: std::sync::Arc<TodoService>,
}

/// 构建 Todo 路由
pub fn todo_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/health", get(health))
        .route("/todos", get(list_todos).post(create_todo))
        .with_state(state)
}

/// 首页
async fn index() -> AppResult<Json<ApiResponse<&'static str>>> {
    Ok(Json(ApiResponse::success("欢迎使用 RyFrame 框架!")))
}

/// 健康检查
async fn health() -> AppResult<Json<ApiResponse<&'static str>>> {
    Ok(Json(ApiResponse::success("ok")))
}

/// 列出所有 Todo
async fn list_todos(State(state): State<AppState>) -> AppResult<Json<ApiResponse<Vec<Todo>>>> {
    let todos = state.todo_service.list_all().await?;
    Ok(Json(ApiResponse::success(todos)))
}

/// 创建 Todo
async fn create_todo(
    State(state): State<AppState>,
    Json(dto): Json<CreateTodoDto>,
) -> AppResult<Json<ApiResponse<Todo>>> {
    dto.validate()
        .map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    let todo = state.todo_service.create(&dto.title).await?;
    Ok(Json(ApiResponse::success(todo)))
}
