use async_trait::async_trait;
use ryframe_common::AppResult;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::task_manager::{ScheduledTask, TaskContext};

/// 清理操作日志任务
///
/// 每天 02:00 删除 30 天前的操作日志
pub struct CleanOperLogTask;

#[async_trait]
impl ScheduledTask for CleanOperLogTask {
    fn name(&self) -> &str {
        "clean_oper_log"
    }

    fn cron(&self) -> &str {
        "0 0 2 * * *"
    }

    fn description(&self) -> &str {
        "每天 02:00 清理 30 天前的操作日志"
    }

    async fn execute(&self, ctx: &TaskContext) -> AppResult<String> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(30);
        let result = ryframe_db::oper_log::Entity::delete_many()
            .filter(ryframe_db::oper_log::Column::OperTime.lt(cutoff))
            .exec(ctx.db.as_ref())
            .await
            .map_err(|e| ryframe_common::AppError::Database(format!("清理操作日志失败: {}", e)))?;

        Ok(format!(
            "清理操作日志完成, 删除 {} 条记录",
            result.rows_affected
        ))
    }
}
