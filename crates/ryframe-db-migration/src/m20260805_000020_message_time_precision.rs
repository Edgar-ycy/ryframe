use sea_orm::{ConnectionTrait, DatabaseBackend};
use sea_orm_migration::prelude::*;

const MESSAGE_TABLE: &str = "sys_message";
const MESSAGE_RECIPIENT_TABLE: &str = "sys_message_recipient";

const ALTER_MESSAGE_TIME_PRECISION_SQL: &str = r#"ALTER TABLE `sys_message`
    MODIFY COLUMN `published_at` DATETIME(6) NOT NULL COMMENT '发布时间',
    MODIFY COLUMN `expires_at` DATETIME(6) DEFAULT NULL COMMENT '过期时间',
    MODIFY COLUMN `created_at` DATETIME(6) NOT NULL COMMENT '创建时间',
    MODIFY COLUMN `updated_at` DATETIME(6) NOT NULL COMMENT '更新时间'"#;

const ALTER_RECIPIENT_TIME_PRECISION_SQL: &str = r#"ALTER TABLE `sys_message_recipient`
    MODIFY COLUMN `created_at` DATETIME(6) NOT NULL COMMENT '收件记录创建时间',
    MODIFY COLUMN `enqueued_at` DATETIME(6) DEFAULT NULL COMMENT '已推送时间',
    MODIFY COLUMN `acked_at` DATETIME(6) DEFAULT NULL COMMENT '已确认时间',
    MODIFY COLUMN `read_at` DATETIME(6) DEFAULT NULL COMMENT '已读时间'"#;

/// 统一消息中心时间精度，避免 MySQL 将小数秒四舍五入到未来一秒。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DatabaseBackend::MySql {
            return Err(DbErr::Custom("消息中心时间精度迁移仅支持 MySQL".into()));
        }

        for table in [MESSAGE_TABLE, MESSAGE_RECIPIENT_TABLE] {
            if !manager.has_table(table).await? {
                return Err(DbErr::Custom(format!(
                    "缺少 {table}，不能统一消息中心时间精度"
                )));
            }
        }

        manager
            .get_connection()
            .execute_unprepared(ALTER_MESSAGE_TIME_PRECISION_SQL)
            .await?;
        manager
            .get_connection()
            .execute_unprepared(ALTER_RECIPIENT_TIME_PRECISION_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "消息中心时间精度迁移不可逆：降级会丢失小数秒并恢复错误舍入语义".into(),
        ))
    }
}
