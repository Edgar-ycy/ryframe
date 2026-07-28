use sea_orm::DatabaseBackend;
use sea_orm_migration::prelude::*;

/// 持久化的数据库后台任务队列。
///
/// 队列有意存放在主 MySQL 数据库中，使任务入队可参与产生业务变更的同一事务。
/// Worker 使用 `FOR UPDATE SKIP LOCKED` 领取记录，因此采用至少一次投递语义，
/// 处理器必须保持幂等。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DatabaseBackend::MySql {
            return Err(DbErr::Custom(
                "background jobs require MySQL 8.0+ with SKIP LOCKED support".into(),
            ));
        }

        if !manager.has_table("sys_background_job").await? {
            manager
                .create_table(
                    Table::create()
                        .table(Alias::new("sys_background_job"))
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
                            ColumnDef::new(Alias::new("job_type"))
                                .string_len(96)
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
                            ColumnDef::new(Alias::new("priority"))
                                .integer()
                                .not_null()
                                .default(0),
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
                            ColumnDef::new(Alias::new("created_at"))
                                .date_time()
                                .not_null(),
                        )
                        .col(
                            ColumnDef::new(Alias::new("updated_at"))
                                .date_time()
                                .not_null(),
                        )
                        .col(
                            ColumnDef::new(Alias::new("completed_at"))
                                .date_time()
                                .null(),
                        )
                        .to_owned(),
                )
                .await?;
        }

        let indexes = [
            (
                "uq_bg_job_dedupe",
                true,
                ["job_type", "dedupe_key"].as_slice(),
            ),
            (
                "idx_bg_job_claim",
                false,
                ["status", "available_at", "priority", "id"].as_slice(),
            ),
            (
                "idx_bg_job_lease",
                false,
                ["status", "lease_until"].as_slice(),
            ),
            (
                "idx_bg_job_tenant",
                false,
                ["tenant_id", "status", "created_at"].as_slice(),
            ),
        ];
        for (name, unique, columns) in indexes {
            if manager.has_index("sys_background_job", name).await? {
                continue;
            }
            let mut index = Index::create();
            index.name(name).table(Alias::new("sys_background_job"));
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
            "background job migration is forward-only; queued work must not be discarded".into(),
        ))
    }
}
