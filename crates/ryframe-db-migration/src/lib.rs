//! MySQL-only schema initialization and versioned upgrades.
//!
//! The baseline migration creates a complete empty installation. Existing v0.4
//! databases are accepted only when every baseline table is present; partially
//! initialized schemas are rejected instead of being silently repaired.

use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbBackend, Statement, TransactionTrait,
    TryGetable,
};
use sea_orm_migration::prelude::*;

mod m20260522_000000_mysql_baseline;
mod m20260625_000001_tenant_completion;
mod m20260701_000002_menu_permission_binding;
mod m20260701_000003_user_auth_version;
mod m20260705_000004_relation_foreign_keys;
mod m20260714_000005_super_role_permissions;
mod schema;
mod seeder;

pub use schema::verify_current_schema;
pub use seeder::{mysql_snapshot_sql, seed};

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260522_000000_mysql_baseline::Migration),
            Box::new(m20260625_000001_tenant_completion::Migration),
            Box::new(m20260701_000002_menu_permission_binding::Migration),
            Box::new(m20260701_000003_user_auth_version::Migration),
            Box::new(m20260705_000004_relation_foreign_keys::Migration),
            Box::new(m20260714_000005_super_role_permissions::Migration),
        ]
    }
}

pub async fn run(db: &DatabaseConnection) -> Result<(), DbErr> {
    if db.get_database_backend() != DatabaseBackend::MySql {
        return Err(DbErr::Custom("RyFrame v0.5 only supports MySQL".into()));
    }
    let transaction = db.begin().await?;
    if let Err(error) = acquire_migration_lock(&transaction).await {
        let _ = transaction.rollback().await;
        return Err(error);
    }
    let migration_result = migrate_seed_verify(&transaction).await;
    let release_result = release_migration_lock(&transaction).await;
    match migration_result.and(release_result) {
        Ok(()) => transaction.commit().await,
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

async fn migrate_seed_verify<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
    for<'c> &'c C: IntoSchemaManagerConnection<'c>,
{
    Migrator::up(db, None)
        .await
        .map_err(|error| DbErr::Custom(format!("migration execution failed: {error}")))?;
    seed(db)
        .await
        .map_err(|error| DbErr::Custom(format!("seed execution failed: {error}")))?;
    verify_current_schema(db)
        .await
        .map_err(|error| DbErr::Custom(format!("schema verification failed: {error}")))
}

async fn acquire_migration_lock<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT GET_LOCK(SHA2(CONCAT('ryframe:v0.5:migration:', DATABASE()), 256), 60)"
                .to_owned(),
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("MySQL migration lock returned no result".into()))?;
    if Option::<i64>::try_get_by_index(&row, 0)? != Some(1) {
        return Err(DbErr::Custom(
            "timed out waiting for the MySQL migration lock".into(),
        ));
    }
    Ok(())
}

async fn release_migration_lock<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT RELEASE_LOCK(SHA2(CONCAT('ryframe:v0.5:migration:', DATABASE()), 256))"
                .to_owned(),
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("MySQL migration lock release returned no result".into()))?;
    if Option::<i64>::try_get_by_index(&row, 0)? != Some(1) {
        return Err(DbErr::Custom(
            "failed to release the MySQL migration lock".into(),
        ));
    }
    Ok(())
}
