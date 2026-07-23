use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_table("sys_file").await? {
            return Ok(());
        }

        if !manager.has_column("sys_file", "upload_status").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("sys_file"))
                        .add_column(
                            ColumnDef::new(Alias::new("upload_status"))
                                .string_len(16)
                                .not_null()
                                .default("ready"),
                        )
                        .to_owned(),
                )
                .await?;
        }
        if !manager.has_column("sys_file", "reservation_token").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("sys_file"))
                        .add_column(
                            ColumnDef::new(Alias::new("reservation_token"))
                                .string_len(64)
                                .null(),
                        )
                        .to_owned(),
                )
                .await?;
        }
        if !manager
            .has_column("sys_file", "reservation_expires_at")
            .await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("sys_file"))
                        .add_column(
                            ColumnDef::new(Alias::new("reservation_expires_at"))
                                .date_time()
                                .null(),
                        )
                        .to_owned(),
                )
                .await?;
        }

        if !manager
            .has_index("sys_file", "idx_file_upload_reservation")
            .await?
        {
            manager
                .create_index(
                    Index::create()
                        .name("idx_file_upload_reservation")
                        .table(Alias::new("sys_file"))
                        .col(Alias::new("tenant_id"))
                        .col(Alias::new("bucket"))
                        .col(Alias::new("file_md5"))
                        .col(Alias::new("upload_status"))
                        .to_owned(),
                )
                .await?;
        }
        if !manager
            .has_index("sys_file", "idx_file_reservation_expiry")
            .await?
        {
            manager
                .create_index(
                    Index::create()
                        .name("idx_file_reservation_expiry")
                        .table(Alias::new("sys_file"))
                        .col(Alias::new("upload_status"))
                        .col(Alias::new("reservation_expires_at"))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_table("sys_file").await? {
            return Ok(());
        }

        for index_name in ["idx_file_reservation_expiry", "idx_file_upload_reservation"] {
            if manager.has_index("sys_file", index_name).await? {
                manager
                    .drop_index(
                        Index::drop()
                            .name(index_name)
                            .table(Alias::new("sys_file"))
                            .to_owned(),
                    )
                    .await?;
            }
        }

        for column_name in [
            "reservation_expires_at",
            "reservation_token",
            "upload_status",
        ] {
            if manager.has_column("sys_file", column_name).await? {
                manager
                    .alter_table(
                        Table::alter()
                            .table(Alias::new("sys_file"))
                            .drop_column(Alias::new(column_name))
                            .to_owned(),
                    )
                    .await?;
            }
        }
        Ok(())
    }
}
