use async_trait::async_trait;
use ryframe_common::AppResult;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::task_manager::{ScheduledTask, TaskContext};

/// 清理登录日志任务
///
/// 每天 03:00 删除 90 天前的登录日志
pub struct CleanLoginInfoTask;

#[async_trait]
impl ScheduledTask for CleanLoginInfoTask {
    fn name(&self) -> &str {
        "clean_login_info"
    }

    fn cron(&self) -> &str {
        "0 0 3 * * *"
    }

    fn description(&self) -> &str {
        "每天 03:00 清理 90 天前的登录日志"
    }

    async fn execute(&self, ctx: &TaskContext) -> AppResult<String> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(90);
        let result = ryframe_db::login_info::Entity::delete_many()
            .filter(ryframe_db::login_info::Column::LoginTime.lt(cutoff))
            .exec(ctx.db.as_ref())
            .await
            .map_err(|e| ryframe_common::AppError::Database(format!("清理登录日志失败: {}", e)))?;

        Ok(format!("清理登录日志完成, 删除 {} 条记录", result.rows_affected))
    }
}
