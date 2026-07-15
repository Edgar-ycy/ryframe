use sea_orm::sea_query::{Expr, Query};
use sea_orm::{ConnectionTrait, DbBackend, Statement, TryGetable};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        backfill_button_menus(manager).await?;
        remove_dynamic_tenant_menus(manager).await?;
        if manager.get_database_backend() != DbBackend::MySql {
            // SQLite 不能通过 ALTER TABLE 为既有表补外键；生产 MySQL 执行下方迁移。
            return Ok(());
        }

        add_fk(
            manager,
            "sys_role_permission",
            "fk_sys_role_permission_role",
            "role_id",
            "sys_role",
            "id",
        )
        .await?;
        add_fk(
            manager,
            "sys_role_permission",
            "fk_sys_role_permission_permission",
            "perm_id",
            "sys_permission",
            "id",
        )
        .await?;
        add_fk(
            manager,
            "sys_role_dept",
            "fk_sys_role_dept_role",
            "role_id",
            "sys_role",
            "id",
        )
        .await?;
        add_fk(
            manager,
            "sys_role_dept",
            "fk_sys_role_dept_dept",
            "dept_id",
            "sys_dept",
            "id",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}

async fn backfill_button_menus(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    if !manager.has_table("sys_permission").await? || !manager.has_table("sys_menu").await? {
        return Ok(());
    }
    let backend = manager.get_database_backend();
    let rows = manager
        .get_connection()
        .query_all_raw(Statement::from_string(
            backend,
            "SELECT p.id, p.tenant_id, p.name, p.code, p.sort, p.status FROM sys_permission p WHERE p.perm_type = 'api' AND p.code NOT LIKE 'tenant:%' AND NOT EXISTS (SELECT 1 FROM sys_menu m WHERE m.tenant_id = p.tenant_id AND m.perm_id = p.id AND m.menu_type = 'F')".to_owned(),
        ))
        .await?;

    for row in rows {
        let perm_id = i64::try_get_by_index(&row, 0)?;
        let tenant_id = String::try_get_by_index(&row, 1)?;
        let name = String::try_get_by_index(&row, 2)?;
        let code = String::try_get_by_index(&row, 3)?;
        let sort = i32::try_get_by_index(&row, 4)?;
        let status = String::try_get_by_index(&row, 5)?;
        let parts = code.split(':').take(2).collect::<Vec<_>>();
        if parts.len() != 2 {
            continue;
        }
        let route_key = parts.join(".");
        let parent = manager
            .get_connection()
            .query_one_raw(Statement::from_sql_and_values(
                backend,
                match backend {
                    DbBackend::Postgres => "SELECT id FROM sys_menu WHERE tenant_id = $1 AND route_key = $2 AND menu_type = 'C' LIMIT 1",
                    _ => "SELECT id FROM sys_menu WHERE tenant_id = ? AND route_key = ? AND menu_type = 'C' LIMIT 1",
                },
                [tenant_id.clone().into(), route_key.into()],
            ))
            .await?;
        let Some(parent_id) = parent
            .map(|item| i64::try_get_by_index(&item, 0))
            .transpose()?
        else {
            continue;
        };
        manager
            .exec_stmt(
                Query::insert()
                    .into_table(Alias::new("sys_menu"))
                    .columns([
                        Alias::new("id"),
                        Alias::new("tenant_id"),
                        Alias::new("name"),
                        Alias::new("parent_id"),
                        Alias::new("menu_type"),
                        Alias::new("perm_id"),
                        Alias::new("route_key"),
                        Alias::new("icon"),
                        Alias::new("sort"),
                        Alias::new("visible"),
                        Alias::new("status"),
                        Alias::new("remark"),
                        Alias::new("del_flag"),
                        Alias::new("created_at"),
                        Alias::new("updated_at"),
                    ])
                    .values_panic([
                        ryframe_common::utils::snowflake::next_snowflake_id().into(),
                        tenant_id.into(),
                        name.into(),
                        parent_id.into(),
                        "F".into(),
                        perm_id.into(),
                        Expr::value(Option::<String>::None),
                        Expr::value(Option::<String>::None),
                        sort.into(),
                        true.into(),
                        status.into(),
                        Expr::value(Option::<String>::None),
                        "0".into(),
                        Expr::current_timestamp(),
                        Expr::current_timestamp(),
                    ])
                    .to_owned(),
            )
            .await?;
    }
    Ok(())
}

async fn remove_dynamic_tenant_menus(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    if !manager.has_table("sys_menu").await? || !manager.has_column("sys_menu", "route_key").await?
    {
        return Ok(());
    }
    if manager.has_column("sys_menu", "parent_id").await? {
        manager
            .get_connection()
            .execute_unprepared(
                "DELETE FROM sys_menu WHERE parent_id IN (SELECT id FROM (SELECT id FROM sys_menu WHERE route_key = 'platform.tenant') tenant_menu)",
            )
            .await?;
    }
    manager
        .get_connection()
        .execute_unprepared("DELETE FROM sys_menu WHERE route_key = 'platform.tenant'")
        .await?;
    Ok(())
}

async fn add_fk(
    manager: &SchemaManager<'_>,
    table: &str,
    name: &str,
    column: &str,
    target_table: &str,
    target_column: &str,
) -> Result<(), DbErr> {
    if !manager.has_table(table).await? || !manager.has_table(target_table).await? {
        return Ok(());
    }
    let row = manager
        .get_connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT COUNT(*) AS count FROM information_schema.TABLE_CONSTRAINTS WHERE CONSTRAINT_SCHEMA = DATABASE() AND TABLE_NAME = ? AND CONSTRAINT_NAME = ? AND CONSTRAINT_TYPE = 'FOREIGN KEY'",
            [table.into(), name.into()],
        ))
        .await?;
    let exists = row
        .map(|row| i64::try_get_by_index(&row, 0))
        .transpose()?
        .unwrap_or(0)
        > 0;
    if exists {
        return Ok(());
    }
    manager
        .create_foreign_key(
            ForeignKey::create()
                .name(name)
                .from(Alias::new(table), Alias::new(column))
                .to(Alias::new(target_table), Alias::new(target_column))
                .on_delete(ForeignKeyAction::Cascade)
                .to_owned(),
        )
        .await
}
