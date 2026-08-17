use sea_orm::DatabaseBackend;
use sea_orm_migration::prelude::*;

const TABLE: &str = "sys_export_job";
const READ_AT_COLUMN: &str = "notification_read_at";
const NOTIFICATION_INDEX: &str = "idx_export_job_notification";

/// 为导出完成提醒补充持久未读状态与定向查询索引。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DatabaseBackend::MySql {
            return Err(DbErr::Custom(
                "export job notifications require MySQL 8.0.16 or newer".into(),
            ));
        }
        if !manager.has_table(TABLE).await? {
            return Err(DbErr::Custom("missing sys_export_job table".into()));
        }
        if !manager.has_column(TABLE, READ_AT_COLUMN).await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new(TABLE))
                        .add_column(
                            ColumnDef::new(Alias::new(READ_AT_COLUMN))
                                .date_time()
                                .null(),
                        )
                        .to_owned(),
                )
                .await?;
        }
        // 即使进程在字段 DDL 提交后异常退出，重跑迁移仍会完成历史回填。
        // 升级前已经进入终态的历史任务默认视为已查看，避免部署后突然产生大量旧提醒。
        manager
            .get_connection()
            .execute_unprepared(
                "UPDATE `sys_export_job` SET `notification_read_at` = COALESCE(`completed_at`, `updated_at`) WHERE `status` IN ('succeeded', 'failed') AND `notification_read_at` IS NULL",
            )
            .await?;
        if !manager.has_index(TABLE, NOTIFICATION_INDEX).await? {
            manager
                .create_index(
                    Index::create()
                        .name(NOTIFICATION_INDEX)
                        .table(Alias::new(TABLE))
                        .col(Alias::new("tenant_id"))
                        .col(Alias::new("requester_id"))
                        .col(Alias::new(READ_AT_COLUMN))
                        .col(Alias::new("status"))
                        .col(Alias::new("completed_at"))
                        .col(Alias::new("id"))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "export job notification migration is forward-only".into(),
        ))
    }
}
