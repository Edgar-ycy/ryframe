use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_common::AppResult;
use serde::Serialize;
use tokio::sync::Mutex;

/// 任务执行历史持久化接口
///
/// 调度器每次执行完任务后调用此 trait 将历史记录写入外部存储（例如数据库）。
#[async_trait]
pub trait TaskHistoryPersister: Send + Sync {
    async fn persist(&self, history: &TaskHistory) -> AppResult<()>;
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskHistory {
    pub task_name: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub cost_ms: i64,
    pub status: String,
    pub message: String,
}

impl TaskHistory {
    pub const STATUS_FAIL: &str = "0";
    pub const STATUS_SUCCESS: &str = "1";
}

#[derive(Clone)]
pub struct TaskHistoryStore {
    inner: Arc<Mutex<Vec<TaskHistory>>>,
    capacity: usize,
}

impl Default for TaskHistoryStore {
    fn default() -> Self {
        Self {
            inner: Default::default(),
            capacity: 500,
        }
    }
}

impl TaskHistoryStore {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Default::default(),
            capacity,
        }
    }

    pub async fn push(&self, h: TaskHistory) {
        let mut inner = self.inner.lock().await;
        inner.push(h);
        if inner.len() > self.capacity {
            inner.remove(0);
        }
    }

    pub async fn recent(&self, name: Option<&str>, limit: usize) -> Vec<TaskHistory> {
        let inner = self.inner.lock().await;
        let iter: Vec<&TaskHistory> = inner.iter().rev().collect();
        let filtered: Vec<&TaskHistory> = if let Some(n) = name {
            iter.into_iter().filter(|h| h.task_name == n).collect()
        } else {
            iter
        };
        filtered.into_iter().take(limit).cloned().collect()
    }

    pub fn clone_store(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            capacity: self.capacity,
        }
    }
}
