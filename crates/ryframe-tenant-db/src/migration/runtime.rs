use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement, TransactionTrait, TryGetable,
};
use sea_orm_migration::prelude::*;

use super::catalog::TENANT_DATA_CATALOG;
use super::{
    schema::{ensure_mysql, verify, verify_mysql_80},
    status::TENANT_DATA_MIGRATION_LEDGER,
};

const MIGRATION_LOCK_SQL_PREFIX: &str = "ryframe:tenant-data-migration:";

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        let mut migrations: Vec<Box<dyn MigrationTrait>> =
            vec![Box::new(super::m20260820_000000_tenant_baseline::Migration)];
        migrations.extend(crate::generated::migrations());
        migrations
    }

    fn migration_table_name() -> DynIden {
        Alias::new(TENANT_DATA_MIGRATION_LEDGER).into_iden()
    }
}

/// 升级一个明确选择的租户数据目标。不会创建任何控制面 `sys_*` 表。
pub async fn up(db: &DatabaseConnection) -> Result<(), DbErr> {
    up_pending_business_migrations(db).await?;
    verify(db).await
}

/// 应用框架租户库迁移但不立即校验业务表。
///
/// 组合根会先调用此入口，再运行已注册业务模块的租户迁移，最后统一校验完整 schema。
pub async fn up_pending_business_migrations(db: &DatabaseConnection) -> Result<(), DbErr> {
    ensure_mysql(db)?;
    verify_mysql_80(db).await?;
    TENANT_DATA_CATALOG
        .validate()
        .map_err(|error| DbErr::Custom(format!("tenant-data catalog is invalid: {error}")))?;

    let transaction = db.begin().await?;
    if let Err(error) = acquire_migration_lock(&transaction).await {
        let _ = transaction.rollback().await;
        return Err(error);
    }
    let migration_result = Migrator::up(&transaction, None)
        .await
        .map_err(|error| DbErr::Custom(format!("tenant-data migration failed: {error}")));
    let release_result = release_migration_lock(&transaction).await;
    match migration_result.and(release_result) {
        Ok(()) => transaction.commit().await?,
        Err(error) => {
            let _ = transaction.rollback().await;
            return Err(error);
        }
    }
    Ok(())
}

async fn acquire_migration_lock<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let value = scalar_i64_on(
        db,
        format!(
            "SELECT GET_LOCK(SHA2(CONCAT('{MIGRATION_LOCK_SQL_PREFIX}', DATABASE()), 256), 60)"
        ),
    )
    .await?;
    if value != 1 {
        return Err(DbErr::Custom(
            "timed out waiting for the tenant-data migration lock".into(),
        ));
    }
    Ok(())
}

async fn release_migration_lock<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let value = scalar_i64_on(
        db,
        format!(
            "SELECT RELEASE_LOCK(SHA2(CONCAT('{MIGRATION_LOCK_SQL_PREFIX}', DATABASE()), 256))"
        ),
    )
    .await?;
    if value != 1 {
        return Err(DbErr::Custom(
            "failed to release the tenant-data migration lock".into(),
        ));
    }
    Ok(())
}

async fn scalar_i64_on<C>(db: &C, sql: String) -> Result<i64, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_string(DbBackend::MySql, sql))
        .await?
        .ok_or_else(|| DbErr::Custom("tenant-data migration lock returned no result".into()))?;
    Option::<i64>::try_get_by_index(&row, 0)?
        .ok_or_else(|| DbErr::Custom("tenant-data migration lock returned NULL".into()))
}
