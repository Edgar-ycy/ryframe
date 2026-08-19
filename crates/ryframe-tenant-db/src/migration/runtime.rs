use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement, TransactionTrait, TryGetable,
};
use sea_orm_migration::prelude::*;

use super::catalog::{TENANT_DATA_CATALOG, TENANT_DATA_SCHEMA_FINGERPRINT};
use super::schema::{ensure_mysql, verify, verify_mysql_80};

pub const TENANT_DATA_MIGRATION_LEDGER: &str = "seaql_tenant_data_migrations";
const MIGRATION_LOCK_SQL_PREFIX: &str = "ryframe:tenant-data-migration:";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationStatus {
    pub applied: usize,
    pub expected: usize,
    pub schema_fingerprint: &'static str,
}

impl MigrationStatus {
    pub fn is_up_to_date(&self) -> bool {
        self.applied == self.expected
    }
}

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(super::m20260817_000001_tenant_fence::Migration),
            Box::new(super::m20260817_000002_reconcile_shared_control_fence::Migration),
            Box::new(super::m20260817_000003_target_slot::Migration),
        ]
    }

    fn migration_table_name() -> DynIden {
        Alias::new(TENANT_DATA_MIGRATION_LEDGER).into_iden()
    }
}

/// 升级一个明确选择的租户数据目标。不会创建任何控制面 `sys_*` 表。
pub async fn up(db: &DatabaseConnection) -> Result<(), DbErr> {
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
    verify(db).await
}

/// 读取独立迁移账本，不执行 DDL。
pub async fn status(db: &DatabaseConnection) -> Result<MigrationStatus, DbErr> {
    ensure_mysql(db)?;
    verify_mysql_80(db).await?;
    let expected = Migrator::migrations().len();
    let ledger_exists = scalar_i64(
        db,
        "SELECT CAST(COUNT(*) AS SIGNED) AS `table_count` FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_name = 'seaql_tenant_data_migrations'",
    )
    .await?
        > 0;
    let applied = if ledger_exists {
        scalar_i64(
            db,
            "SELECT CAST(COUNT(*) AS SIGNED) FROM seaql_tenant_data_migrations",
        )
        .await? as usize
    } else {
        0
    };
    Ok(MigrationStatus {
        applied,
        expected,
        schema_fingerprint: TENANT_DATA_SCHEMA_FINGERPRINT,
    })
}

async fn scalar_i64(db: &DatabaseConnection, sql: &str) -> Result<i64, DbErr> {
    scalar_i64_statement(db, Statement::from_string(DbBackend::MySql, sql.to_owned())).await
}

async fn scalar_i64_statement(db: &DatabaseConnection, statement: Statement) -> Result<i64, DbErr> {
    let row = db
        .query_one_raw(statement)
        .await?
        .ok_or_else(|| DbErr::Custom("tenant-data verification query returned no result".into()))?;
    Option::<i64>::try_get_by_index(&row, 0)?.ok_or_else(|| {
        DbErr::Custom("tenant-data verification query returned a NULL scalar".into())
    })
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
