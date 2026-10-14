use super::{BUSINESS_MIGRATION_LEDGER_DDL, BusinessMigration};
use ryframe_kernel::{AppError, AppResult};

/// 按注册顺序生成业务结构和账本，不访问数据库、不写入文件。
pub fn business_initialization_statements(
    migrations: &[&dyn BusinessMigration],
) -> AppResult<Vec<String>> {
    if migrations.is_empty() {
        return Ok(Vec::new());
    }
    super::schema::validate_migrations(migrations)?;
    let mut statements = vec![BUSINESS_MIGRATION_LEDGER_DDL.into()];
    for migration in migrations {
        let source = migration.initialization_statements()?;
        if source.is_empty() || source.iter().any(|sql| sql.trim().is_empty()) {
            return Err(AppError::Config(format!(
                "业务迁移 {}/{} 的初始化 SQL 为空；提供与 up 同源的结构语句，不能只登记账本",
                migration.module(),
                migration.id()
            )));
        }
        statements.extend(source.iter().map(|sql| (*sql).to_owned()));
        let before = migration
            .schema()
            .before_fingerprint
            .map(sql_text)
            .unwrap_or_else(|| "NULL".into());
        statements.push(format!(
            "INSERT INTO ryframe_business_migration (module_name, migration_id, checksum, before_fingerprint, after_fingerprint) VALUES ({}, {}, {}, {}, {})",
            sql_text(migration.module()), sql_text(migration.id()), sql_text(migration.checksum()), before, sql_text(migration.schema().after_fingerprint),
        ));
    }
    Ok(statements)
}

fn sql_text(value: &str) -> String {
    // 十六进制字面量与 NO_BACKSLASH_ESCAPES 等模式无关，不拼接可执行文本。
    let hex = value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("CONVERT(X'{hex}' USING utf8mb4)")
}
