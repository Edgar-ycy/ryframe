use async_trait::async_trait;
use ryframe_common::AppResult;
use sea_orm::DatabaseConnection;
use std::sync::Arc;

/// 任务执行上下文（传递给 execute）
#[derive(Clone)]
pub struct TaskContext {
    pub db: Arc<DatabaseConnection>,
}

/// 调度任务 trait
#[async_trait]
pub trait ScheduledTask: Send + Sync {
    /// 任务唯一标识
    fn name(&self) -> &str;

    /// Cron 表达式（6 段：sec min hour day month week）
    fn cron(&self) -> &str;

    /// 任务说明
    fn description(&self) -> &str { "" }

    /// 执行任务，返回结果消息
    async fn execute(&self, ctx: &TaskContext) -> AppResult<String>;
}