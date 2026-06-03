use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::model::Todo;

/// Todo 数据访问 trait（面向接口编程）
#[async_trait]
pub trait TodoRepository: Send + Sync + 'static {
    /// 查找全部 Todo
    async fn find_all(&self) -> Vec<Todo>;

    /// 新增 Todo 并返回更新后的模型
    async fn insert(&self, todo: Todo) -> Todo;
}

/// 基于内存的 TodoRepository 实现
pub struct InMemoryTodoRepository {
    todos: Arc<Mutex<Vec<Todo>>>,
}

impl InMemoryTodoRepository {
    pub fn new(todos: Vec<Todo>) -> Self {
        Self {
            todos: Arc::new(Mutex::new(todos)),
        }
    }
}

#[async_trait]
impl TodoRepository for InMemoryTodoRepository {
    async fn find_all(&self) -> Vec<Todo> {
        self.todos.lock().await.clone()
    }

    async fn insert(&self, todo: Todo) -> Todo {
        let mut todos = self.todos.lock().await;
        todos.push(todo.clone());
        todo
    }
}
