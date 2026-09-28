use std::collections::BTreeSet;

use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, DbErr, FromQueryResult, Statement, TryGetable,
};

use super::schema::{ensure_mysql, verify_mysql_80};

pub const TENANT_DATA_MIGRATION_LEDGER: &str = "seaql_tenant_data_migrations";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationStatus {
    pub applied: usize,
    pub expected: usize,
    pub missing: Vec<String>,
    pub unexpected: Vec<String>,
    pub schema_fingerprint: &'static str,
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
            schema_fingerprint: super::tenant_data_schema_fingerprint(),
        }
    }
}

#[derive(Debug, FromQueryResult)]
struct MigrationVersionRow {
    version: String,
}

/// 读取独立迁移账本，不执行 DDL。
pub async fn status(db: &DatabaseConnection) -> Result<MigrationStatus, DbErr> {
    ensure_mysql(db)?;
    verify_mysql_80(db).await?;
    status_after_server_validation(db).await
}

pub(super) async fn status_after_server_validation(
    db: &DatabaseConnection,
) -> Result<MigrationStatus, DbErr> {
    let ledger_exists = scalar_i64(
        db,
        "SELECT CAST(COUNT(*) AS SIGNED) AS `table_count` FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_name = 'seaql_tenant_data_migrations'",
    )
    .await?
        > 0;
    let applied_versions = if ledger_exists {
        MigrationVersionRow::find_by_statement(Statement::from_string(
            DbBackend::MySql,
            "SELECT version FROM seaql_tenant_data_migrations ORDER BY version",
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
        super::expected_migration_names()
            .map(str::to_owned)
            .collect(),
    ))
}

async fn scalar_i64(db: &DatabaseConnection, sql: &str) -> Result<i64, DbErr> {
    let row = db
        .query_one_raw(Statement::from_string(DbBackend::MySql, sql.to_owned()))
        .await?
        .ok_or_else(|| DbErr::Custom("tenant-data verification query returned no result".into()))?;
    Option::<i64>::try_get_by_index(&row, 0)?.ok_or_else(|| {
        DbErr::Custom("tenant-data verification query returned a NULL scalar".into())
    })
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
