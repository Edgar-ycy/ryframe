//! 仅支持 MySQL 的控制库新基线。

use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbBackend, FromQueryResult, Statement,
    TransactionTrait, TryGetable,
};
use sea_orm_migration::prelude::*;

mod m20260820_000000_control_baseline;
mod schema;
mod seeder;

pub use schema::verify_current_schema;
pub use seeder::{mysql_snapshot_sql, seed};

const MIGRATION_LOCK_SQL_PREFIX: &str = "ryframe:migration:";
pub const CONTROL_MIGRATION_LEDGER: &str = "seaql_migrations";

/// 迁移账本状态，适用于部署 CLI 和就绪报告。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationStatus {
    pub applied: usize,
    pub expected: usize,
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
        vec![Box::new(m20260820_000000_control_baseline::Migration)]
    }

    fn migration_table_name() -> DynIden {
        Alias::new(CONTROL_MIGRATION_LEDGER).into_iden()
    }
}

/// 应用待执行迁移，幂等地初始化系统数据，并校验 schema。
///
/// 这是唯一允许执行 DDL 的操作，供独立部署任务使用，而非生产 API 启动过程。
pub async fn up(db: &DatabaseConnection) -> Result<(), DbErr> {
    ensure_mysql(db)?;
    verify_mysql_80(db).await?;
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

/// 在不执行 DDL 或初始化写入的情况下，校验迁移账本完整且主库 schema 与当前迁移
/// 指纹相匹配。
pub async fn verify(db: &DatabaseConnection) -> Result<(), DbErr> {
    ensure_mysql(db)?;
    verify_mysql_80(db).await?;
    let status = status(db).await?;
    if !status.is_up_to_date() {
        return Err(DbErr::Custom(format!(
            "control migration ledger is not current: applied {}, expected {}; run `ryframe-migrate control up` before starting the API",
            status.applied, status.expected
        )));
    }
    verify_current_schema(db)
        .await
        .map_err(|error| DbErr::Custom(format!("schema verification failed: {error}")))
}

/// 在不改变数据库状态的情况下读取迁移账本状态。
pub async fn status(db: &DatabaseConnection) -> Result<MigrationStatus, DbErr> {
    ensure_mysql(db)?;
    verify_mysql_80(db).await?;
    let expected = Migrator::migrations().len();
    let ledger_exists = scalar_i64(
        db,
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_name = 'seaql_migrations'",
    )
    .await?
        > 0;
    let applied = if ledger_exists {
        scalar_i64(db, "SELECT COUNT(*) FROM seaql_migrations").await? as usize
    } else {
        0
    };
    Ok(MigrationStatus { applied, expected })
}

#[derive(Debug, FromQueryResult)]
struct ServerIdentityRow {
    version: String,
    version_comment: String,
}

/// 仅接受支持受约束 CHECK 的 MySQL 8.0.16 或更高版本。
async fn verify_mysql_80(db: &DatabaseConnection) -> Result<(), DbErr> {
    let identity = ServerIdentityRow::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        "SELECT VERSION() AS version, @@version_comment AS version_comment",
    ))
    .one(db)
    .await?
    .ok_or_else(|| DbErr::Custom("cannot verify MySQL server identity".into()))?;
    let supported = supports_mysql_80_or_newer(&identity.version, &identity.version_comment);
    if !supported {
        return Err(DbErr::Custom(
            "RyFrame requires MySQL 8.0.16 or newer".into(),
        ));
    }
    Ok(())
}

fn supports_mysql_80_or_newer(version: &str, version_comment: &str) -> bool {
    if version.to_ascii_lowercase().contains("mariadb")
        || version_comment.to_ascii_lowercase().contains("mariadb")
    {
        return false;
    }

    let version_core = version.split(['-', '+']).next().unwrap_or_default();
    let mut parts = version_core.split('.');
    let Some(major) = parts.next().and_then(|part| part.parse::<u32>().ok()) else {
        return false;
    };
    let Some(minor) = parts.next().and_then(|part| part.parse::<u32>().ok()) else {
        return false;
    };
    let Some(patch) = parts.next().and_then(|part| part.parse::<u32>().ok()) else {
        return false;
    };

    (major, minor, patch) >= (8, 0, 16)
}

fn ensure_mysql(db: &DatabaseConnection) -> Result<(), DbErr> {
    if db.get_database_backend() != DatabaseBackend::MySql {
        return Err(DbErr::Custom("RyFrame supports MySQL only".into()));
    }
    Ok(())
}

async fn scalar_i64(db: &DatabaseConnection, sql: &str) -> Result<i64, DbErr> {
    let row = db
        .query_one_raw(Statement::from_string(DbBackend::MySql, sql.to_owned()))
        .await?
        .ok_or_else(|| DbErr::Custom(format!("query returned no result: {sql}")))?;
    Option::<i64>::try_get_by_index(&row, 0)?
        .ok_or_else(|| DbErr::Custom(format!("query returned a NULL scalar value: {sql}")))
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
            format!(
                "SELECT GET_LOCK(SHA2(CONCAT('{MIGRATION_LOCK_SQL_PREFIX}', DATABASE()), 256), 60)"
            ),
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
            format!(
                "SELECT RELEASE_LOCK(SHA2(CONCAT('{MIGRATION_LOCK_SQL_PREFIX}', DATABASE()), 256))"
            ),
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
