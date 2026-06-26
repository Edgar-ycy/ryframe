//! Versioned schema upgrades run before the HTTP server starts.
//! Migrations are deliberately additive so an existing single-tenant database
//! can be upgraded in place and all historical records become `system` data.

use sea_orm::DatabaseConnection;
use sea_orm_migration::prelude::*;

mod m20260625_000001_tenant_completion;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20260625_000001_tenant_completion::Migration)]
    }
}

pub async fn run(db: &DatabaseConnection) -> Result<(), DbErr> {
    Migrator::up(db, None).await
}

#[cfg(test)]
mod tests {
    use sea_orm::Database;
    use sea_orm_migration::prelude::SchemaManager;

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
}
