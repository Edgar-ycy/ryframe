//! Versioned schema upgrades run before the HTTP server starts.
//! Migrations are deliberately additive so an existing single-tenant database
//! can be upgraded in place and all historical records become `system` data.

use sea_orm::DatabaseConnection;
use sea_orm_migration::prelude::*;

mod m20260625_000001_tenant_completion;
mod m20260701_000002_menu_permission_binding;
mod m20260701_000003_user_auth_version;
mod m20260705_000004_relation_foreign_keys;
mod m20260714_000005_super_role_permissions;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260625_000001_tenant_completion::Migration),
            Box::new(m20260701_000002_menu_permission_binding::Migration),
            Box::new(m20260701_000003_user_auth_version::Migration),
            Box::new(m20260705_000004_relation_foreign_keys::Migration),
            Box::new(m20260714_000005_super_role_permissions::Migration),
        ]
    }
}

pub async fn run(db: &DatabaseConnection) -> Result<(), DbErr> {
    Migrator::up(db, None).await
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Database, Statement, TryGetable};
    use sea_orm_migration::prelude::{Alias, ColumnDef, Index, SchemaManager, Table};

    use super::run;

    #[tokio::test]
    async fn sqlite_migration_creates_tenant_schema() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        run(&db).await.unwrap();
        assert!(
            SchemaManager::new(&db)
                .has_table("sys_tenant")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn sqlite_migration_adds_menu_binding_columns() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        let manager = SchemaManager::new(&db);
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("sys_menu"))
                    .col(ColumnDef::new(Alias::new("id")).big_integer().primary_key())
                    .col(
                        ColumnDef::new(Alias::new("tenant_id"))
                            .string_len(64)
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("name")).string_len(64).not_null())
                    .col(
                        ColumnDef::new(Alias::new("menu_type"))
                            .char_len(1)
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await
            .unwrap();
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("sys_user"))
                    .col(ColumnDef::new(Alias::new("id")).big_integer().primary_key())
                    .to_owned(),
            )
            .await
            .unwrap();
        run(&db).await.unwrap();
        assert!(manager.has_column("sys_menu", "perm_id").await.unwrap());
        assert!(manager.has_column("sys_menu", "route_key").await.unwrap());
        assert!(
            manager
                .has_column("sys_user", "auth_version")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn sqlite_migration_accepts_preexisting_menu_binding_indexes() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        let manager = SchemaManager::new(&db);
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("sys_menu"))
                    .col(ColumnDef::new(Alias::new("id")).big_integer().primary_key())
                    .col(
                        ColumnDef::new(Alias::new("tenant_id"))
                            .string_len(64)
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("name")).string_len(64).not_null())
                    .col(
                        ColumnDef::new(Alias::new("menu_type"))
                            .char_len(1)
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("perm_id")).big_integer().null())
                    .col(
                        ColumnDef::new(Alias::new("route_key"))
                            .string_len(100)
                            .null(),
                    )
                    .to_owned(),
            )
            .await
            .unwrap();
        for (name, column) in [
            ("idx_menu_tenant_perm", "perm_id"),
            ("idx_menu_tenant_route", "route_key"),
        ] {
            manager
                .create_index(
                    Index::create()
                        .name(name)
                        .table(Alias::new("sys_menu"))
                        .col(Alias::new("tenant_id"))
                        .col(Alias::new(column))
                        .to_owned(),
                )
                .await
                .unwrap();
        }

        run(&db).await.unwrap();

        assert!(
            manager
                .has_index("sys_menu", "idx_menu_tenant_perm")
                .await
                .unwrap()
        );
        assert!(
            manager
                .has_index("sys_menu", "idx_menu_tenant_route")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn sqlite_migration_backfills_all_api_buttons_and_skips_static_tenant_menu() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        db.execute_unprepared("CREATE TABLE sys_menu (id BIGINT PRIMARY KEY, tenant_id TEXT NOT NULL, name TEXT NOT NULL, parent_id BIGINT NULL, menu_type TEXT NOT NULL, perm_id BIGINT NULL, route_key TEXT NULL, icon TEXT NULL, sort INTEGER NOT NULL, visible BOOLEAN NOT NULL, status TEXT NOT NULL, remark TEXT NULL, del_flag TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL)").await.unwrap();
        db.execute_unprepared("CREATE TABLE sys_permission (id BIGINT PRIMARY KEY, tenant_id TEXT NOT NULL, name TEXT NOT NULL, code TEXT NOT NULL, parent_id BIGINT NULL, perm_type TEXT NOT NULL, icon TEXT NULL, sort INTEGER NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL)").await.unwrap();
        db.execute_unprepared("INSERT INTO sys_menu VALUES (10, 'system', '用户管理', NULL, 'C', 1, 'system.user', 'User', 1, 1, '1', NULL, '0', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP), (20, 'system', '租户管理', NULL, 'C', 3, 'platform.tenant', NULL, 2, 1, '1', NULL, '0', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)").await.unwrap();
        db.execute_unprepared("INSERT INTO sys_permission VALUES (1, 'system', '用户查询', 'system:user:list', NULL, 'api', NULL, 1, '1', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP), (2, 'system', '用户新增', 'system:user:add', NULL, 'api', NULL, 2, '1', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP), (3, 'system', '租户查询', 'tenant:list', NULL, 'api', NULL, 1, '1', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)").await.unwrap();

        run(&db).await.unwrap();
        let row = db
            .query_one_raw(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                "SELECT COUNT(*) FROM sys_menu WHERE menu_type = 'F'".to_owned(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(i64::try_get_by_index(&row, 0).unwrap(), 2);
        let row = db
            .query_one_raw(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                "SELECT COUNT(*) FROM sys_menu WHERE route_key = 'platform.tenant'".to_owned(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(i64::try_get_by_index(&row, 0).unwrap(), 0);
    }
}
