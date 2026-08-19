use sea_orm::{ConnectionTrait, DatabaseBackend};
use sea_orm_migration::prelude::*;

const TABLE: &str = "sys_message_recipient";
const LEGACY_INBOX_INDEX: &str = "idx_message_recipient_inbox";
const LEGACY_ACK_INDEX: &str = "idx_message_recipient_ack";
const VISIBLE_INDEX: &str = "idx_message_recipient_visible";
const UNREAD_INDEX: &str = "idx_message_recipient_unread";
const UNACKED_INDEX: &str = "idx_message_recipient_unacked";

const CURRENT_SNAPSHOT_TABLE_SQL: &str = r####"CREATE TABLE IF NOT EXISTS `sys_message_recipient` (
    `message_id` BIGINT NOT NULL COMMENT '消息ID',
    `user_id` BIGINT NOT NULL COMMENT '收件用户ID',
    `tenant_id` VARCHAR(64) NOT NULL COMMENT '租户标识',
    `created_at` DATETIME(6) NOT NULL COMMENT '收件记录创建时间',
    `enqueued_at` DATETIME(6) DEFAULT NULL COMMENT '已推送时间',
    `acked_at` DATETIME(6) DEFAULT NULL COMMENT '已确认时间',
    `read_at` DATETIME(6) DEFAULT NULL COMMENT '已读时间',
    `deleted_at` DATETIME(6) DEFAULT NULL COMMENT '收件人删除时间',
    PRIMARY KEY (`message_id`, `user_id`),
    KEY `idx_message_recipient_visible` (`tenant_id`, `user_id`, `deleted_at`, `message_id` DESC),
    KEY `idx_message_recipient_unread` (`tenant_id`, `user_id`, `deleted_at`, `read_at`, `message_id` DESC),
    KEY `idx_message_recipient_unacked` (`tenant_id`, `user_id`, `deleted_at`, `acked_at`, `message_id` DESC),
    CONSTRAINT `fk_message_recipient_message`
        FOREIGN KEY (`message_id`) REFERENCES `sys_message` (`id`)
        ON UPDATE CASCADE ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='消息收件箱表'"####;

/// 将历史基线中的收件箱建表语句映射为执行完本迁移后的当前快照。
pub(crate) fn current_snapshot_statement(statement: &'static str) -> &'static str {
    if statement
        .trim_start()
        .starts_with("CREATE TABLE IF NOT EXISTS `sys_message_recipient`")
    {
        CURRENT_SNAPSHOT_TABLE_SQL
    } else {
        statement
    }
}

/// 增加收件人级软删除，并安装已通过代表性数据执行计划验证的收件箱索引。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        ensure_mysql_table(manager).await?;
        if !manager.has_column(TABLE, "deleted_at").await? {
            manager
                .get_connection()
                .execute_unprepared(
                    "ALTER TABLE `sys_message_recipient` \
                     ADD COLUMN `deleted_at` DATETIME(6) DEFAULT NULL COMMENT '收件人删除时间' AFTER `read_at`",
                )
                .await?;
        }
        create_index_if_missing(
            manager,
            VISIBLE_INDEX,
            "CREATE INDEX `idx_message_recipient_visible` ON `sys_message_recipient` \
             (`tenant_id`, `user_id`, `deleted_at`, `message_id` DESC)",
        )
        .await?;
        create_index_if_missing(
            manager,
            UNREAD_INDEX,
            "CREATE INDEX `idx_message_recipient_unread` ON `sys_message_recipient` \
             (`tenant_id`, `user_id`, `deleted_at`, `read_at`, `message_id` DESC)",
        )
        .await?;
        create_index_if_missing(
            manager,
            UNACKED_INDEX,
            "CREATE INDEX `idx_message_recipient_unacked` ON `sys_message_recipient` \
             (`tenant_id`, `user_id`, `deleted_at`, `acked_at`, `message_id` DESC)",
        )
        .await?;
        drop_index_if_present(manager, LEGACY_INBOX_INDEX).await?;
        drop_index_if_present(manager, LEGACY_ACK_INDEX).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        ensure_mysql_table(manager).await?;
        create_index_if_missing(
            manager,
            LEGACY_INBOX_INDEX,
            "CREATE INDEX `idx_message_recipient_inbox` ON `sys_message_recipient` \
             (`tenant_id`, `user_id`, `read_at`, `created_at`, `message_id`)",
        )
        .await?;
        create_index_if_missing(
            manager,
            LEGACY_ACK_INDEX,
            "CREATE INDEX `idx_message_recipient_ack` ON `sys_message_recipient` \
             (`tenant_id`, `user_id`, `acked_at`, `message_id`)",
        )
        .await?;
        drop_index_if_present(manager, VISIBLE_INDEX).await?;
        drop_index_if_present(manager, UNREAD_INDEX).await?;
        drop_index_if_present(manager, UNACKED_INDEX).await?;
        if manager.has_column(TABLE, "deleted_at").await? {
            manager
                .get_connection()
                .execute_unprepared("ALTER TABLE `sys_message_recipient` DROP COLUMN `deleted_at`")
                .await?;
        }
        Ok(())
    }
}

async fn ensure_mysql_table(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    if manager.get_database_backend() != DatabaseBackend::MySql {
        return Err(DbErr::Custom("消息收件人软删除迁移仅支持 MySQL".into()));
    }
    if !manager.has_table(TABLE).await? {
        return Err(DbErr::Custom(format!("缺少 {TABLE}，无法增加软删除")));
    }
    Ok(())
}

async fn create_index_if_missing(
    manager: &SchemaManager<'_>,
    name: &str,
    sql: &str,
) -> Result<(), DbErr> {
    if !manager.has_index(TABLE, name).await? {
        manager.get_connection().execute_unprepared(sql).await?;
    }
    Ok(())
}

async fn drop_index_if_present(manager: &SchemaManager<'_>, name: &str) -> Result<(), DbErr> {
    if manager.has_index(TABLE, name).await? {
        manager
            .get_connection()
            .execute_unprepared(&format!("DROP INDEX `{name}` ON `{TABLE}`"))
            .await?;
    }
    Ok(())
}
