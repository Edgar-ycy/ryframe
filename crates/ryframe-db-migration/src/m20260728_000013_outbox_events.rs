use sea_orm::DatabaseBackend;
use sea_orm_migration::prelude::*;

/// 创建事务 Outbox 事件表。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DatabaseBackend::MySql {
            return Err(DbErr::Custom("outbox events require MySQL 8.0+".into()));
        }
        if manager.has_table("sys_outbox_event").await? {
            return Ok(());
        }
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("sys_outbox_event"))
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Alias::new("id"))
                            .big_integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("tenant_id"))
                            .string_len(64)
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("event_type"))
                            .string_len(96)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("aggregate_type"))
                            .string_len(64)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("aggregate_id"))
                            .string_len(128)
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("payload")).json().not_null())
                    .col(
                        ColumnDef::new(Alias::new("status"))
                            .string_len(16)
                            .not_null()
                            .default("pending"),
                    )
                    .col(
                        ColumnDef::new(Alias::new("available_at"))
                            .date_time()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("attempts"))
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Alias::new("max_attempts"))
                            .integer()
                            .not_null()
                            .default(5),
                    )
                    .col(
                        ColumnDef::new(Alias::new("lease_owner"))
                            .string_len(128)
                            .null(),
                    )
                    .col(ColumnDef::new(Alias::new("lease_until")).date_time().null())
                    .col(
                        ColumnDef::new(Alias::new("dedupe_key"))
                            .string_len(191)
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("traceparent"))
                            .string_len(255)
                            .null(),
                    )
                    .col(ColumnDef::new(Alias::new("last_error")).text().null())
                    .col(
                        ColumnDef::new(Alias::new("published_at"))
                            .date_time()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("created_at"))
                            .date_time()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("updated_at"))
                            .date_time()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        for (name, unique, columns) in [
            (
                "uq_outbox_event_dedupe",
                true,
                ["event_type", "dedupe_key"].as_slice(),
            ),
            (
                "idx_outbox_event_claim",
                false,
                ["status", "available_at", "id"].as_slice(),
            ),
            (
                "idx_outbox_event_lease",
                false,
                ["status", "lease_until"].as_slice(),
            ),
            (
                "idx_outbox_event_aggregate",
                false,
                ["aggregate_type", "aggregate_id", "created_at"].as_slice(),
            ),
        ] {
            let mut index = Index::create();
            index.name(name).table(Alias::new("sys_outbox_event"));
            if unique {
                index.unique();
            }
            for column in columns {
                index.col(Alias::new(*column));
            }
            manager.create_index(index.to_owned()).await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "outbox event migration is forward-only".into(),
        ))
    }
}
