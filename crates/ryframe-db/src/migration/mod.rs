//! 仅支持 MySQL 的控制库新基线。

use std::collections::BTreeSet;

#[cfg(feature = "migration")]
use sea_orm::TransactionTrait;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbBackend, DbErr, FromQueryResult,
    Statement, TryGetable,
};
#[cfg(feature = "migration")]
use sea_orm_migration::prelude::*;

mod access_catalog;
mod baseline_contract;
#[cfg(feature = "migration")]
mod m20260820_000000_control_baseline;
mod schema;
#[cfg(feature = "migration")]
mod seeder;

pub use access_catalog::{
    AccessMenu, access_menus, access_permission_codes, access_permission_names,
};
pub use baseline_contract::ddl_statements as control_ddl_statements;
pub use schema::{
    expected_extra, extract_column_type, normalize_check_clause, normalize_column_type,
    verify_current_schema,
};
#[cfg(feature = "migration")]
pub use seeder::{mysql_snapshot_sql, seed, validate_seed_statements};

#[cfg(feature = "migration")]
const MIGRATION_LOCK_SQL_PREFIX: &str = "ryframe:migration:";
pub const CONTROL_MIGRATION_LEDGER: &str = "seaql_migrations";
const HANDWRITTEN_MIGRATION_NAMES: &[&str] = &["m20260820_000000_control_baseline"];

pub fn expected_migration_names() -> impl Iterator<Item = &'static str> {
    HANDWRITTEN_MIGRATION_NAMES
        .iter()
        .copied()
        .chain(crate::generated::MIGRATION_NAMES.iter().copied())
}

/// 迁移账本状态，适用于部署 CLI 和就绪报告。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationStatus {
    pub applied: usize,
    pub expected: usize,
    pub missing: Vec<String>,
    pub unexpected: Vec<String>,
}

impl MigrationStatus {
    pub fn is_up_to_date(&self) -> bool {
        self.applied == self.expected && self.missing.is_empty() && self.unexpected.is_empty()
    }

    fn from_versions(applied_versions: Vec<String>, expected_versions: Vec<String>) -> Self {
        let applied_names = applied_versions
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let expected_names = expected_versions
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let missing = expected_names
            .difference(&applied_names)
            .map(|name| (*name).to_owned())
            .collect();
        let unexpected = applied_names
            .difference(&expected_names)
            .map(|name| (*name).to_owned())
            .collect();
        Self {
            applied: applied_versions.len(),
            expected: expected_versions.len(),
            missing,
            unexpected,
        }
    }
}

#[cfg(feature = "migration")]
pub struct Migrator;

/// 当前唯一控制库 baseline 的稳定 schema 指纹。
pub fn schema_fingerprint() -> String {
    baseline_contract::schema_fingerprint()
}

#[cfg(feature = "migration")]
#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        let mut migrations: Vec<Box<dyn MigrationTrait>> =
            vec![Box::new(m20260820_000000_control_baseline::Migration)];
        migrations.extend(crate::generated::migrations());
        migrations
    }

    fn migration_table_name() -> DynIden {
        Alias::new(CONTROL_MIGRATION_LEDGER).into_iden()
    }
}

/// 应用待执行迁移，幂等地初始化系统数据，并校验 schema。
///
/// 这是唯一允许执行 DDL 的操作，供独立部署任务使用，而非生产 API 启动过程。
#[cfg(feature = "migration")]
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
    let status = status(db).await?;
    if !status.is_up_to_date() {
        return Err(DbErr::Custom(format!(
            "control migration ledger is not current: applied {}, expected {}, missing [{}], unexpected [{}]; run `ryframe-migrate control up` before starting the API",
            status.applied,
            status.expected,
            status.missing.join(","),
            status.unexpected.join(",")
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
    let ledger_exists = scalar_i64(
        db,
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_name = 'seaql_migrations'",
    )
    .await?
        > 0;
    let applied_versions = if ledger_exists {
        MigrationVersionRow::find_by_statement(Statement::from_string(
            DbBackend::MySql,
            "SELECT version FROM seaql_migrations ORDER BY version",
        ))
        .all(db)
        .await?
        .into_iter()
        .map(|migration| migration.version)
        .collect()
    } else {
        Vec::new()
    };
    Ok(MigrationStatus::from_versions(
        applied_versions,
        expected_migration_names().map(str::to_owned).collect(),
    ))
}

#[derive(Debug, FromQueryResult)]
struct ServerIdentityRow {
    version: String,
    version_comment: String,
}

#[derive(Debug, FromQueryResult)]
struct MigrationVersionRow {
    version: String,
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

/// 判断服务器身份是否满足控制面和租户数据面共同要求的 MySQL 最低版本。
#[must_use]
pub fn supports_mysql_80_or_newer(version: &str, version_comment: &str) -> bool {
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

#[cfg(feature = "migration")]
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

#[cfg(feature = "migration")]
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

#[cfg(feature = "migration")]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_status_uses_exact_names_not_only_count() {
        let current =
            MigrationStatus::from_versions(vec!["baseline".into()], vec!["baseline".into()]);
        assert!(current.is_up_to_date());
        assert!(current.missing.is_empty());
        assert!(current.unexpected.is_empty());

        let mismatched = MigrationStatus::from_versions(
            vec!["baseline".into(), "wrong".into()],
            vec!["baseline".into(), "expected".into()],
        );
        assert_eq!(mismatched.applied, mismatched.expected);
        assert_eq!(mismatched.missing, ["expected"]);
        assert_eq!(mismatched.unexpected, ["wrong"]);
        assert!(!mismatched.is_up_to_date());

        let missing = MigrationStatus::from_versions(Vec::new(), vec!["baseline".into()]);
        assert_eq!(missing.missing, ["baseline"]);
        assert!(missing.unexpected.is_empty());
        assert!(!missing.is_up_to_date());

        let unexpected = MigrationStatus::from_versions(vec!["unknown".into()], Vec::new());
        assert!(unexpected.missing.is_empty());
        assert_eq!(unexpected.unexpected, ["unknown"]);
        assert!(!unexpected.is_up_to_date());

        let duplicate = MigrationStatus::from_versions(
            vec!["baseline".into(), "baseline".into()],
            vec!["baseline".into()],
        );
        assert!(duplicate.missing.is_empty());
        assert!(duplicate.unexpected.is_empty());
        assert!(!duplicate.is_up_to_date());
    }
}
