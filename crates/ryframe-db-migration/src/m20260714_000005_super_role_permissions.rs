use sea_orm::{ConnectionTrait, DbBackend, Statement, TryGetable};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        add_is_super_column(manager).await?;
        backfill_super_role_flag(manager).await?;
        backfill_super_only_permissions(manager).await?;
        backfill_super_only_button_menus(manager).await?;
        drop_sys_user_role_foreign_keys(manager).await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}

async fn add_is_super_column(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    if !manager.has_table("sys_role").await? || manager.has_column("sys_role", "is_super").await? {
        return Ok(());
    }

    manager
        .alter_table(
            Table::alter()
                .table(Alias::new("sys_role"))
                .add_column(
                    ColumnDef::new(Alias::new("is_super"))
                        .tiny_integer()
                        .not_null()
                        .default(0),
                )
                .to_owned(),
        )
        .await
}

async fn backfill_super_role_flag(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    if !manager.has_table("sys_role").await? || !manager.has_column("sys_role", "is_super").await? {
        return Ok(());
    }

    manager
        .get_connection()
        .execute_unprepared(
            "UPDATE sys_role SET is_super = CASE WHEN code = 'admin' THEN 1 ELSE 0 END",
        )
        .await?;
    Ok(())
}

async fn backfill_super_only_permissions(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    if !manager.has_table("sys_permission").await? {
        return Ok(());
    }

    let specs = [
        ("sys:user:editSelf", "编辑自身角色", "system:user", 7),
        ("sys:role:editSuper", "编辑超级管理员角色", "system:role", 6),
    ];
    for (code, name, parent_code, sort) in specs {
        let parents = manager
            .get_connection()
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                "SELECT tenant_id, id FROM sys_permission WHERE code = ?",
                [parent_code.into()],
            ))
            .await?;

        for parent in parents {
            let tenant_id = String::try_get_by_index(&parent, 0)?;
            let parent_id = i64::try_get_by_index(&parent, 1)?;
            if permission_exists(manager, &tenant_id, code).await? {
                continue;
            }
            manager
                .exec_stmt(
                    Query::insert()
                        .into_table(Alias::new("sys_permission"))
                        .columns([
                            Alias::new("id"),
                            Alias::new("tenant_id"),
                            Alias::new("name"),
                            Alias::new("code"),
                            Alias::new("parent_id"),
                            Alias::new("perm_type"),
                            Alias::new("icon"),
                            Alias::new("sort"),
                            Alias::new("status"),
                            Alias::new("created_at"),
                            Alias::new("updated_at"),
                        ])
                        .values_panic([
                            ryframe_common::utils::snowflake::next_snowflake_id().into(),
                            tenant_id.into(),
                            name.into(),
                            code.into(),
                            parent_id.into(),
                            "api".into(),
                            sea_orm::sea_query::Expr::value(Option::<String>::None),
                            sort.into(),
                            "1".into(),
                            sea_orm::sea_query::Expr::current_timestamp(),
                            sea_orm::sea_query::Expr::current_timestamp(),
                        ])
                        .to_owned(),
                )
                .await?;
        }
    }

    Ok(())
}

async fn backfill_super_only_button_menus(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    if !manager.has_table("sys_permission").await? || !manager.has_table("sys_menu").await? {
        return Ok(());
    }

    let specs = [
        ("sys:user:editSelf", "system.user", 7),
        ("sys:role:editSuper", "system.role", 6),
    ];
    for (code, route_key, sort) in specs {
        let rows = manager
            .get_connection()
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                "SELECT p.id, p.tenant_id, p.name, m.id FROM sys_permission p JOIN sys_menu m ON m.tenant_id = p.tenant_id AND m.route_key = ? AND m.menu_type = 'C' WHERE p.code = ?",
                [route_key.into(), code.into()],
            ))
            .await?;

        for row in rows {
            let perm_id = i64::try_get_by_index(&row, 0)?;
            let tenant_id = String::try_get_by_index(&row, 1)?;
            let name = String::try_get_by_index(&row, 2)?;
            let parent_id = i64::try_get_by_index(&row, 3)?;
            if button_menu_exists(manager, &tenant_id, perm_id).await? {
                continue;
            }
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
                            sea_orm::sea_query::Expr::value(Option::<String>::None),
                            sea_orm::sea_query::Expr::value(Option::<String>::None),
                            sort.into(),
                            true.into(),
                            "1".into(),
                            sea_orm::sea_query::Expr::value(Option::<String>::None),
                            "0".into(),
                            sea_orm::sea_query::Expr::current_timestamp(),
                            sea_orm::sea_query::Expr::current_timestamp(),
                        ])
                        .to_owned(),
                )
                .await?;
        }
    }

    Ok(())
}

async fn permission_exists(
    manager: &SchemaManager<'_>,
    tenant_id: &str,
    code: &str,
) -> Result<bool, DbErr> {
    let row = manager
        .get_connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT COUNT(*) FROM sys_permission WHERE tenant_id = ? AND code = ?",
            [tenant_id.into(), code.into()],
        ))
        .await?;
    Ok(row
        .map(|row| i64::try_get_by_index(&row, 0))
        .transpose()?
        .unwrap_or(0)
        > 0)
}

async fn button_menu_exists(
    manager: &SchemaManager<'_>,
    tenant_id: &str,
    perm_id: i64,
) -> Result<bool, DbErr> {
    let row = manager
        .get_connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT COUNT(*) FROM sys_menu WHERE tenant_id = ? AND perm_id = ? AND menu_type = 'F'",
            [tenant_id.into(), perm_id.into()],
        ))
        .await?;
    Ok(row
        .map(|row| i64::try_get_by_index(&row, 0))
        .transpose()?
        .unwrap_or(0)
        > 0)
}

async fn drop_sys_user_role_foreign_keys(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    if !manager.has_table("sys_user_role").await? {
        return Ok(());
    }

    for name in [
        "fk_sys_user_role_tenant",
        "fk_sys_user_role_user",
        "fk_sys_user_role_role",
    ] {
        if mysql_foreign_key_exists(manager, "sys_user_role", name).await? {
            manager
                .drop_foreign_key(
                    ForeignKey::drop()
                        .name(name)
                        .table(Alias::new("sys_user_role"))
                        .to_owned(),
                )
                .await?;
        }
    }

    Ok(())
}

async fn mysql_foreign_key_exists(
    manager: &SchemaManager<'_>,
    table: &str,
    name: &str,
) -> Result<bool, DbErr> {
    let row = manager
        .get_connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT COUNT(*) FROM information_schema.TABLE_CONSTRAINTS WHERE CONSTRAINT_SCHEMA = DATABASE() AND TABLE_NAME = ? AND CONSTRAINT_NAME = ? AND CONSTRAINT_TYPE = 'FOREIGN KEY'",
            [table.into(), name.into()],
        ))
        .await?;
    Ok(row
        .map(|row| i64::try_get_by_index(&row, 0))
        .transpose()?
        .unwrap_or(0)
        > 0)
}
