use std::sync::{Arc, atomic::AtomicU64};

use axum::{Json, Router, extract::State, routing::get};
use ryframe_common::{ApiResponse, AppError, AppResult};
use serde::{Deserialize, Serialize};
use tracing::info;

// ==================== 数据模型 ====================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Todo {
    id: u64,
    title: String,
    done: bool,
}

#[derive(Debug, Deserialize)]
struct CreateTodo {
    title: String,
}

// ==================== 应用状态 ====================

#[derive(Clone)]
struct AppState {
    counter: Arc<AtomicU64>,
    todos: Arc<tokio::sync::Mutex<Vec<Todo>>>,
}

// ==================== Handler ====================

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
    let todos = state.todos.lock().await;
    Ok(Json(ApiResponse::success(todos.clone())))
}

/// 创建 Todo
async fn create_todo(
    State(state): State<AppState>,
    Json(input): Json<CreateTodo>,
) -> AppResult<Json<ApiResponse<Todo>>> {
    if input.title.trim().is_empty() {
        return Err(AppError::Validation("标题不能为空".into()));
    }
    let id = state
        .counter
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        + 1;
    let todo = Todo {
        id,
        title: input.title,
        done: false,
    };
    state.todos.lock().await.push(todo.clone());
    info!("创建 Todo: id={}, title={}", id, todo.title);
    Ok(Json(ApiResponse::success(todo)))
}

#[tokio::main]
async fn main() {
    // 1. 初始化日志
    tracing_subscriber::fmt().with_target(false).init();

    info!("RyFrame 示例程序启动中...");

    // 2. 构建应用状态
    let state = AppState {
        counter: Arc::new(AtomicU64::new(0)),
        todos: Arc::new(tokio::sync::Mutex::new(vec![Todo {
            id: 0,
            title: "学习 RyFrame".into(),
            done: true,
        }])),
    };

    // 3. 构建路由
    let app = Router::new()
        .route("/", get(index))
        .route("/health", get(health))
        .route("/todos", get(list_todos).post(create_todo))
        .with_state(state);

    // 4. 绑定地址
    let addr = "127.0.0.1:3000";
    info!("服务监听: http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
