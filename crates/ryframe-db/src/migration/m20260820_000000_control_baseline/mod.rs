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

pub fn ddl_statements() -> impl Iterator<Item = &'static str> {
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
