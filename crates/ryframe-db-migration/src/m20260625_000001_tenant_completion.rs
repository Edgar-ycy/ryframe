use sea_orm::{ConnectionTrait, DbBackend, Statement, TryGetable};
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

        for (table, index, foreign_key) in [
            (
                "sys_login_info",
                "idx_tenant_id",
                "fk_sys_login_info_tenant",
            ),
            ("sys_oper_log", "idx_tenant_id", "fk_sys_oper_log_tenant"),
            (
                "password_reset_requests",
                "idx_password_reset_tenant",
                "fk_password_reset_tenant",
            ),
        ] {
            complete_tenant_binding(manager, table, index, foreign_key).await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // The migration backfills production data; rolling it back must not
        // silently discard tenant ownership information.
        Ok(())
    }
}

async fn complete_tenant_binding(
    manager: &SchemaManager<'_>,
    table: &str,
    index: &str,
    foreign_key: &str,
) -> Result<(), DbErr> {
    if !manager.has_table(table).await? {
        return Ok(());
    }
    if !manager.has_column(table, "tenant_id").await? {
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
    }
    if !manager.has_index(table, index).await? {
        manager
            .create_index(
                Index::create()
                    .name(index)
                    .table(Alias::new(table))
                    .col(Alias::new("tenant_id"))
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;
    }
    if !foreign_key_exists(manager, table, foreign_key).await? {
        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name(foreign_key)
                    .from(Alias::new(table), Alias::new("tenant_id"))
                    .to(Alias::new("sys_tenant"), Alias::new("tenant_id"))
                    .on_update(ForeignKeyAction::Cascade)
                    .on_delete(ForeignKeyAction::Restrict)
                    .to_owned(),
            )
            .await?;
    }
    Ok(())
}

async fn foreign_key_exists(
    manager: &SchemaManager<'_>,
    table: &str,
    name: &str,
) -> Result<bool, DbErr> {
    let row = manager
        .get_connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT COUNT(*) FROM information_schema.TABLE_CONSTRAINTS \
             WHERE CONSTRAINT_SCHEMA = DATABASE() AND TABLE_NAME = ? \
             AND CONSTRAINT_NAME = ? AND CONSTRAINT_TYPE = 'FOREIGN KEY'",
            [table.into(), name.into()],
        ))
        .await?;
    Ok(row
        .map(|row| i64::try_get_by_index(&row, 0))
        .transpose()?
        .unwrap_or(0)
        > 0)
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
