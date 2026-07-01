//! Versioned schema upgrades run before the HTTP server starts.
//! Migrations are deliberately additive so an existing single-tenant database
//! can be upgraded in place and all historical records become `system` data.

use sea_orm::DatabaseConnection;
use sea_orm_migration::prelude::*;

mod m20260625_000001_tenant_completion;
mod m20260701_000002_menu_permission_binding;
mod m20260701_000003_user_auth_version;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260625_000001_tenant_completion::Migration),
            Box::new(m20260701_000002_menu_permission_binding::Migration),
            Box::new(m20260701_000003_user_auth_version::Migration),
        ]
    }
}

pub async fn run(db: &DatabaseConnection) -> Result<(), DbErr> {
    Migrator::up(db, None).await
}

#[cfg(test)]
mod tests {
    use sea_orm::Database;
    use sea_orm_migration::prelude::{Alias, ColumnDef, SchemaManager, Table};

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
}
