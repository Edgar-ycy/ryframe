use sea_orm::DatabaseBackend;
use sea_orm_migration::prelude::*;

const BACKGROUND_JOB_TABLE: &str = "sys_background_job";
const OUTBOX_EVENT_TABLE: &str = "sys_outbox_event";
const TRACE_STATE_COLUMN: &str = "tracestate";

/// 为持久化任务与 Outbox 事件补齐完整的 W3C Trace Context。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DatabaseBackend::MySql {
            return Err(DbErr::Custom("持久化 Trace Context 仅支持 MySQL".into()));
        }

        for table in [BACKGROUND_JOB_TABLE, OUTBOX_EVENT_TABLE] {
            if !manager.has_table(table).await? {
                return Err(DbErr::Custom(format!(
                    "缺少 {table}，不能增加 W3C tracestate"
                )));
            }
            if manager.has_column(table, TRACE_STATE_COLUMN).await? {
                continue;
            }
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new(table))
                        .add_column(
                            ColumnDef::new(Alias::new(TRACE_STATE_COLUMN))
                                .string_len(512)
                                .null()
                                .comment("W3C Trace Context 状态"),
                        )
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "持久化 Trace Context 迁移不可逆：回滚会破坏跨进程链路状态".into(),
        ))
    }
}
