use serde::{Deserialize, Serialize};

/// Todo 领域模型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Todo {
    pub id: u64,
    pub title: String,
    pub done: bool,
}
