use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_table("sys_tenant").await? {
            manager
                .create_table(
                    Table::create()
                        .table(Alias::new("sys_tenant"))
                        .if_not_exists()
                        .col(
                            ColumnDef::new(Tenant::Id)
                                .big_integer()
                                .not_null()
                                .primary_key(),
                        )
                        .col(
                            ColumnDef::new(Alias::new("tenant_id"))
                                .string_len(64)
                                .not_null()
                                .unique_key(),
                        )
                        .col(ColumnDef::new(Tenant::Name).string_len(128).not_null())
                        .col(ColumnDef::new(Tenant::Domain).string_len(255).null())
                        .col(
                            ColumnDef::new(Tenant::Status)
                                .char_len(1)
                                .not_null()
                                .default("1"),
                        )
                        .col(
                            ColumnDef::new(Tenant::ExpireAt)
                                .timestamp_with_time_zone()
                                .null(),
                        )
                        .col(
                            ColumnDef::new(Tenant::MaxUsers)
                                .integer()
                                .not_null()
                                .default(100),
                        )
                        .col(
                            ColumnDef::new(Tenant::MaxRoles)
                                .integer()
                                .not_null()
                                .default(20),
                        )
                        .col(
                            ColumnDef::new(Tenant::MaxStorageMb)
                                .big_integer()
                                .not_null()
                                .default(1024),
                        )
                        .col(
                            ColumnDef::new(Tenant::MaxRequestsPerMin)
                                .integer()
                                .not_null()
                                .default(1000),
                        )
                        .col(
                            ColumnDef::new(Tenant::SessionVersion)
                                .integer()
                                .not_null()
                                .default(1),
                        )
                        .col(
                            ColumnDef::new(Tenant::CreatedAt)
                                .timestamp_with_time_zone()
                                .not_null()
                                .default(Expr::current_timestamp()),
                        )
                        .col(
                            ColumnDef::new(Tenant::UpdatedAt)
                                .timestamp_with_time_zone()
                                .not_null()
                                .default(Expr::current_timestamp()),
                        )
                        .to_owned(),
                )
                .await?;
            manager
                .get_connection()
                .execute_unprepared(
                    "INSERT INTO sys_tenant (id, tenant_id, name, status, session_version) VALUES (1, 'system', '系统租户', '1', 1)",
                )
                .await?;
        } else {
            add_integer_column_if_missing(manager, "sys_tenant", "session_version", 1).await?;
        }

        for table in ["sys_login_info", "sys_oper_log", "password_reset_requests"] {
            add_tenant_column_if_missing(manager, table).await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // The migration backfills production data; rolling it back must not
        // silently discard tenant ownership information.
        Ok(())
    }
}

async fn add_tenant_column_if_missing(
    manager: &SchemaManager<'_>,
    table: &str,
) -> Result<(), DbErr> {
    if manager.has_table(table).await? && !manager.has_column(table, "tenant_id").await? {
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new(table))
                    .add_column(
                        ColumnDef::new(Alias::new("tenant_id"))
                            .string_len(64)
                            .not_null()
                            .default("system"),
                    )
                    .to_owned(),
            )
            .await?;
        let index_name = format!("idx_{}_tenant", table);
        manager
            .create_index(
                Index::create()
                    .name(&index_name)
                    .table(Alias::new(table))
                    .col(Alias::new("tenant_id"))
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;
    }
    Ok(())
}

async fn add_integer_column_if_missing(
    manager: &SchemaManager<'_>,
    table: &str,
    column: &str,
    default: i32,
) -> Result<(), DbErr> {
    if !manager.has_column(table, column).await? {
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new(table))
                    .add_column(
                        ColumnDef::new(Alias::new(column))
                            .integer()
                            .not_null()
                            .default(default),
                    )
                    .to_owned(),
            )
            .await?;
    }
    Ok(())
}

#[derive(DeriveIden)]
enum Tenant {
    Id,
    Name,
    Domain,
    Status,
    ExpireAt,
    MaxUsers,
    MaxRoles,
    MaxStorageMb,
    MaxRequestsPerMin,
    SessionVersion,
    CreatedAt,
    UpdatedAt,
}
