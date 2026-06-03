mod dto;
mod handler;
mod model;
mod repository;
mod service;

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use tracing::info;

use crate::handler::{AppState, todo_router};
use crate::model::Todo;
use crate::repository::InMemoryTodoRepository;
use crate::service::TodoService;

#[tokio::main]
async fn main() {
    // 1. 初始化日志
    tracing_subscriber::fmt().with_target(false).init();

    info!("RyFrame 示例程序启动中...");

    // 2. 构建 repository（内存实现，注入初始数据）
    let repo = Arc::new(InMemoryTodoRepository::new(vec![Todo {
        id: 0,
        title: "学习 RyFrame".into(),
        done: true,
    }]));

    // 3. 构建 service（通过构造函数注入 repository trait 对象）
    let counter = Arc::new(AtomicU64::new(0));
    let todo_service = Arc::new(TodoService::new(repo, counter));

    // 4. 构建应用状态
    let state = AppState { todo_service };

    // 5. 组装路由
    let app = todo_router(state);

    // 6. 启动服务
    let addr = "127.0.0.1:3000";
    info!("服务监听: http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
