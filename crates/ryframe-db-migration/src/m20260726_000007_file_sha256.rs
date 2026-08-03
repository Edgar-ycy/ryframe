use sea_orm_migration::prelude::*;

/// 以可空列补充权威的 SHA-256 内容摘要，供独立维护命令分批回填。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_table("sys_file").await? {
            return Ok(());
        }

        if !manager.has_column("sys_file", "file_sha256").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("sys_file"))
                        .add_column(
                            ColumnDef::new(Alias::new("file_sha256"))
                                .char_len(64)
                                .null(),
                        )
                        .to_owned(),
                )
                .await?;
        }
        if !manager.has_index("sys_file", "idx_file_sha256").await? {
            manager
                .create_index(
                    Index::create()
                        .name("idx_file_sha256")
                        .table(Alias::new("sys_file"))
                        .col(Alias::new("tenant_id"))
                        .col(Alias::new("bucket"))
                        .col(Alias::new("file_sha256"))
                        .col(Alias::new("upload_status"))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "file SHA-256 migration is forward-only; do not discard integrity metadata".into(),
        ))
    }
}
