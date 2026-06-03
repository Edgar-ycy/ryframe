use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use ryframe_common::{AppError, AppResult};
use tracing::info;

use crate::model::Todo;
use crate::repository::TodoRepository;

/// Todo 业务服务
///
/// 通过构造函数注入 `Arc<dyn TodoRepository>` trait 对象，
/// 实现面向接口编程，便于单元测试时替换 Mock 实现。
pub struct TodoService {
    repo: Arc<dyn TodoRepository>,
    counter: Arc<AtomicU64>,
}

impl TodoService {
    /// 构造函数：注入 repository 和计数器
    pub fn new(repo: Arc<dyn TodoRepository>, counter: Arc<AtomicU64>) -> Self {
        Self { repo, counter }
    }

    /// 查询所有 Todo
    pub async fn list_all(&self) -> AppResult<Vec<Todo>> {
        Ok(self.repo.find_all().await)
    }

    /// 创建 Todo
    pub async fn create(&self, title: &str) -> AppResult<Todo> {
        if title.trim().is_empty() {
            return Err(AppError::Validation("标题不能为空".into()));
        }

        let id = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        let todo = Todo {
            id,
            title: title.to_string(),
            done: false,
        };

        let created = self.repo.insert(todo).await;
        info!("创建 Todo: id={}, title={}", created.id, created.title);
        Ok(created)
    }
}
