use sea_orm::DatabaseBackend;
use sea_orm_migration::prelude::*;

mod base;
mod schema;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260820_000000_control_baseline"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DatabaseBackend::MySql {
            return Err(DbErr::Custom(
                "control baseline requires MySQL 8.0.16 or newer".into(),
            ));
        }
        let connection = manager.get_connection();
        let existing_tables = crate::migration::schema::user_tables(connection).await?;
        if !existing_tables.is_empty() {
            return Err(DbErr::Custom(format!(
                "control database is not empty; refusing fresh baseline; existing tables: {}",
                existing_tables.join(", ")
            )));
        }

        for statement in ddl_statements() {
            connection.execute_unprepared(statement).await?;
        }
        for statement in POST_TABLE_STATEMENTS {
            connection.execute_unprepared(statement).await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "control baseline is destructive and cannot be rolled back".into(),
        ))
    }
}

pub(crate) fn ddl_statements() -> impl Iterator<Item = &'static str> {
    base::BASELINE_STATEMENTS
        .iter()
        .copied()
        .filter(|statement| !is_seed_statement(statement))
        .chain(schema::lifecycle_table_statements())
        .chain(schema::tenant_config_table_statements())
        .chain(schema::product_capability_table_statements())
        .chain(schema::tenant_data_control_table_statements())
        .chain(schema::service_account_table_statements())
        .chain([
            schema::OUTBOX_EVENT_DDL,
            schema::EXPORT_JOB_DDL,
            schema::RESOURCE_OWNERSHIP_DDL,
        ])
}

pub(crate) fn seed_statements() -> impl Iterator<Item = &'static str> {
    base::BASELINE_STATEMENTS
        .iter()
        .copied()
        .filter(|statement| is_seed_statement(statement))
}

pub(crate) fn schema_fingerprint() -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for statement in ddl_statements() {
        for byte in statement.trim().bytes().chain([0xff]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

fn is_seed_statement(statement: &str) -> bool {
    statement.trim_start().starts_with("INSERT INTO")
}

const POST_TABLE_STATEMENTS: &[&str] = &[r#"ALTER TABLE `sys_user`
    ADD CONSTRAINT `fk_user_avatar_file`
    FOREIGN KEY (`avatar_file_id`) REFERENCES `sys_file` (`id`)
    ON UPDATE CASCADE ON DELETE RESTRICT"#];

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm_migration::prelude::MigratorTrait;

    #[test]
    fn control_schema_is_one_fresh_baseline() {
        let migrations = crate::migration::Migrator::migrations();
        assert_eq!(migrations.len(), 1);
        assert_eq!(
            crate::migration::CONTROL_MIGRATION_LEDGER,
            "seaql_migrations"
        );
        assert_eq!(migrations[0].name(), "m20260820_000000_control_baseline");
    }

    #[test]
    fn baseline_contains_export_snapshot_and_task_versions() {
        let statements = ddl_statements().collect::<Vec<_>>();
        let export = statements
            .iter()
            .find(|statement| statement.contains("CREATE TABLE IF NOT EXISTS `sys_export_job`"))
            .expect("export table must exist in the baseline");
        let background = statements
            .iter()
            .find(|statement| statement.contains("CREATE TABLE IF NOT EXISTS `sys_background_job`"))
            .expect("background job table must exist in the baseline");
        for column in [
            "request_version",
            "authorization_fingerprint",
            "request_fingerprint",
            "active_request_fingerprint",
            "snapshot_at",
            "upper_id",
            "matched_rows",
            "exported_rows",
            "delete_pending_at",
        ] {
            assert!(export.contains(&format!("`{column}`")));
        }
        assert!(background.contains("`payload_version`"));
    }

    #[test]
    fn baseline_table_set_and_schema_fingerprint_are_stable() {
        let mut tables = ddl_statements()
            .map(|statement| {
                statement
                    .split('`')
                    .nth(1)
                    .expect("baseline statement must name a table")
            })
            .collect::<Vec<_>>();
        let count = tables.len();
        tables.sort_unstable();
        tables.dedup();
        assert_eq!(tables.len(), count);
        assert_eq!(count, 51);
        assert_eq!(schema_fingerprint(), "595a420d869c5fdb");
    }
}
