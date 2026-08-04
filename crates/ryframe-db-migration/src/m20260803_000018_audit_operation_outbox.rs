use sea_orm_migration::prelude::*;

const OPER_LOG_TABLE: &str = "sys_oper_log";
const EVENT_ID_COLUMN: &str = "event_id";
const REQUEST_ID_COLUMN: &str = "request_id";
const EVENT_ID_UNIQUE_INDEX: &str = "uq_oper_log_event_id";

/// 为操作日志增加 Outbox 事件幂等键与请求关联标识。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_table(OPER_LOG_TABLE).await? {
            return Err(DbErr::Custom(
                "缺少 sys_oper_log，不能增加审计 Outbox 标识".into(),
            ));
        }

        if !manager.has_column(OPER_LOG_TABLE, EVENT_ID_COLUMN).await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new(OPER_LOG_TABLE))
                        .add_column(
                            ColumnDef::new(Alias::new(EVENT_ID_COLUMN))
                                .char_len(36)
                                .null()
                                .comment("审计事件 UUID v7"),
                        )
                        .to_owned(),
                )
                .await?;
        }

        if !manager
            .has_column(OPER_LOG_TABLE, REQUEST_ID_COLUMN)
            .await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new(OPER_LOG_TABLE))
                        .add_column(
                            ColumnDef::new(Alias::new(REQUEST_ID_COLUMN))
                                .char_len(36)
                                .null()
                                .comment("HTTP 请求 UUID v7"),
                        )
                        .to_owned(),
                )
                .await?;
        }

        if !manager
            .has_index(OPER_LOG_TABLE, EVENT_ID_UNIQUE_INDEX)
            .await?
        {
            manager
                .create_index(
                    Index::create()
                        .name(EVENT_ID_UNIQUE_INDEX)
                        .table(Alias::new(OPER_LOG_TABLE))
                        .col(Alias::new(EVENT_ID_COLUMN))
                        .unique()
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "操作审计 Outbox 迁移不可逆：回滚会破坏事件幂等性与请求追踪".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{EVENT_ID_COLUMN, EVENT_ID_UNIQUE_INDEX, REQUEST_ID_COLUMN};

    #[test]
    fn audit_migration_uses_stable_column_and_unique_index_names() {
        assert_eq!(EVENT_ID_COLUMN, "event_id");
        assert_eq!(REQUEST_ID_COLUMN, "request_id");
        assert_eq!(EVENT_ID_UNIQUE_INDEX, "uq_oper_log_event_id");
    }
}
