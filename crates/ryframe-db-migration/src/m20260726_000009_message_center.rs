use sea_orm::{ConnectionTrait, DatabaseBackend};
use sea_orm_migration::prelude::*;

/// 增加消息中心表和用户语言偏好。该迁移只新增可兼容对象，不删除历史数据。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DatabaseBackend::MySql {
            return Err(DbErr::Custom("消息中心仅支持 MySQL".into()));
        }

        if manager.has_table("sys_user").await?
            && !manager.has_column("sys_user", "preferred_locale").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("sys_user"))
                        .add_column(
                            ColumnDef::new(Alias::new("preferred_locale"))
                                .string_len(16)
                                .null(),
                        )
                        .to_owned(),
                )
                .await?;
        }

        for statement in MESSAGE_TABLES {
            manager
                .get_connection()
                .execute_unprepared(statement)
                .await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "消息中心迁移为仅向前兼容迁移，不能删除已投递的消息记录".into(),
        ))
    }
}

const MESSAGE_TABLES: &[&str] = &[
    r#"CREATE TABLE IF NOT EXISTS `sys_message` (
        `id` BIGINT NOT NULL,
        `tenant_id` VARCHAR(64) NOT NULL,
        `topic` VARCHAR(64) NOT NULL,
        `title_text` VARCHAR(200) DEFAULT NULL,
        `body_text` TEXT DEFAULT NULL,
        `title_key` VARCHAR(128) DEFAULT NULL,
        `body_key` VARCHAR(128) DEFAULT NULL,
        `args_json` JSON DEFAULT NULL,
        `severity` VARCHAR(16) NOT NULL,
        `payload_json` JSON DEFAULT NULL,
        `source_type` VARCHAR(64) DEFAULT NULL,
        `source_id` VARCHAR(128) DEFAULT NULL,
        `created_by` BIGINT DEFAULT NULL,
        `published_at` DATETIME(6) NOT NULL,
        `expires_at` DATETIME(6) DEFAULT NULL,
        `created_at` DATETIME(6) NOT NULL,
        `updated_at` DATETIME(6) NOT NULL,
        PRIMARY KEY (`id`),
        UNIQUE KEY `uq_message_source` (`tenant_id`, `source_type`, `source_id`),
        KEY `idx_message_tenant_published` (`tenant_id`, `published_at`, `id`),
        KEY `idx_message_expires_at` (`expires_at`)
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci"#,
    r#"CREATE TABLE IF NOT EXISTS `sys_message_audience` (
        `message_id` BIGINT NOT NULL,
        `tenant_id` VARCHAR(64) NOT NULL,
        `kind` VARCHAR(16) NOT NULL,
        `target_id` BIGINT NOT NULL,
        PRIMARY KEY (`message_id`, `kind`, `target_id`),
        KEY `idx_message_audience_tenant` (`tenant_id`, `kind`, `target_id`),
        CONSTRAINT `fk_message_audience_message`
            FOREIGN KEY (`message_id`) REFERENCES `sys_message` (`id`)
            ON UPDATE CASCADE ON DELETE CASCADE
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci"#,
    r#"CREATE TABLE IF NOT EXISTS `sys_message_recipient` (
        `message_id` BIGINT NOT NULL,
        `user_id` BIGINT NOT NULL,
        `tenant_id` VARCHAR(64) NOT NULL,
        `created_at` DATETIME(6) NOT NULL,
        `enqueued_at` DATETIME(6) DEFAULT NULL,
        `acked_at` DATETIME(6) DEFAULT NULL,
        `read_at` DATETIME(6) DEFAULT NULL,
        PRIMARY KEY (`message_id`, `user_id`),
        KEY `idx_message_recipient_inbox` (`tenant_id`, `user_id`, `read_at`, `created_at`, `message_id`),
        KEY `idx_message_recipient_ack` (`tenant_id`, `user_id`, `acked_at`, `message_id`),
        CONSTRAINT `fk_message_recipient_message`
            FOREIGN KEY (`message_id`) REFERENCES `sys_message` (`id`)
            ON UPDATE CASCADE ON DELETE CASCADE
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci"#,
];
