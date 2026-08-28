use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement, TryGetable};

use super::schema::{ensure_mysql, verify_mysql_80};

pub const TENANT_DATA_MIGRATION_LEDGER: &str = "seaql_tenant_data_migrations";

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

/// 读取独立迁移账本，不执行 DDL。
pub async fn status(db: &DatabaseConnection) -> Result<MigrationStatus, DbErr> {
    ensure_mysql(db)?;
    verify_mysql_80(db).await?;
    let expected = super::expected_migration_names().count();
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
        schema_fingerprint: super::TENANT_DATA_SCHEMA_FINGERPRINT,
    })
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
