use chrono::{DateTime, Utc};
use ryframe_kernel::AppResult;

/// 一次实体写入共用的自动填充上下文。
pub struct FillContext {
    pub now: DateTime<Utc>,
    pub user_id: Option<i64>,
    pub username: Option<String>,
}

impl FillContext {
    pub fn new() -> Self {
        Self {
            now: Utc::now(),
            user_id: None,
            username: None,
        }
    }
}

impl Default for FillContext {
    fn default() -> Self {
        Self::new()
    }
}

/// 控制库实体在写入前执行的自动填充行为。
pub trait AutoFill {
    fn fill_on_insert(&mut self, context: &FillContext) -> AppResult<()>;
    fn fill_on_update(&mut self, context: &FillContext) -> AppResult<()>;
}
