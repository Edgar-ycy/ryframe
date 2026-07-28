use sea_orm::DatabaseBackend;
use sea_orm_migration::prelude::*;

/// 创建面向用户的异步导出任务表。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DatabaseBackend::MySql {
            return Err(DbErr::Custom("export jobs require MySQL 8.0+".into()));
        }
        if manager.has_table("sys_export_job").await? {
            return Ok(());
        }
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("sys_export_job"))
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
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("requester_id"))
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("resource"))
                            .string_len(64)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("background_job_id"))
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("request_params"))
                            .json()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("permission_code"))
                            .string_len(128)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("status"))
                            .string_len(16)
                            .not_null()
                            .default("queued"),
                    )
                    .col(
                        ColumnDef::new(Alias::new("result_file_id"))
                            .big_integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("result_file_name"))
                            .string_len(255)
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("content_type"))
                            .string_len(128)
                            .null(),
                    )
                    .col(ColumnDef::new(Alias::new("file_size")).big_integer().null())
                    .col(ColumnDef::new(Alias::new("expires_at")).date_time().null())
                    .col(ColumnDef::new(Alias::new("error_message")).text().null())
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
        for (name, unique, columns) in [
            (
                "uq_export_job_background",
                true,
                ["background_job_id"].as_slice(),
            ),
            (
                "idx_export_job_requester",
                false,
                ["tenant_id", "requester_id", "created_at"].as_slice(),
            ),
            (
                "idx_export_job_expiry",
                false,
                ["status", "expires_at"].as_slice(),
            ),
        ] {
            let mut index = Index::create();
            index.name(name).table(Alias::new("sys_export_job"));
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
        Err(DbErr::Custom("export job migration is forward-only".into()))
    }
}
