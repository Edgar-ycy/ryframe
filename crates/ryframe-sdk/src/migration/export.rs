use super::BusinessMigration;
use ryframe_kernel::{AppError, AppResult};

mod guard;

/// DDL 阶段与最终登记事务分离；组合根在 verification 后登记框架完成标记。
#[derive(Debug)]
pub struct BusinessInitializationSql {
    pub setup: Vec<String>,
    pub migrations: Vec<String>,
    pub verification: Vec<String>,
    pub finish: Vec<String>,
}

impl BusinessInitializationSql {
    /// 独立 SDK 场景在完整核验后提交；组合根必须先插入自己的框架完成标记。
    pub fn into_statements(self) -> Vec<String> {
        self.setup
            .into_iter()
            .chain(self.migrations)
            .chain(self.verification)
            .chain(self.finish)
            .collect()
    }
}

/// 初始化 SQL 不解释业务 DDL；每次实际结构核验成功后才登记该迁移。
pub fn business_initialization_statements(
    migrations: &[&dyn BusinessMigration],
    framework: &[(String, ryframe_db::schema::SchemaAssertions)],
) -> AppResult<BusinessInitializationSql> {
    super::schema::validate_migrations(migrations)?;
    guard::validate_owners(migrations)?;
    let mut setup = vec![
        "SET @ryframe_previous_cte_depth=@@SESSION.cte_max_recursion_depth".into(),
        "SET SESSION cte_max_recursion_depth=4294967295".into(),
        ryframe_db::schema::schema_assertion_guard().into(),
    ];
    if !migrations.is_empty() {
        setup.push(super::ledger::DDL.into());
    }
    let mut statements = Vec::new();
    for (position, migration) in migrations.iter().enumerate() {
        statements.extend(guard::before(
            *migration,
            &migrations[..position],
            framework,
        )?);
        let source = migration.initialization_statements()?;
        if source.is_empty() || source.iter().any(|sql| sql.trim().is_empty()) {
            return Err(AppError::Config(format!(
                "业务迁移 {}/{} 的初始化 SQL 为空；提供与 up 同源的结构语句，不能只登记账本",
                migration.module(),
                migration.id()
            )));
        }
        statements.extend(source.iter().flat_map(|sql| {
            ryframe_db::connection::generated_mysql_ddl_statements(&strict_create(sql))
        }));
        statements.extend(guard::after(
            &migrations[..=position],
            &migrations[..position],
            framework,
        )?);
        statements.push(ledger_insert(*migration)?);
        statements.push("COMMIT".into());
    }
    let verification = guard::after(migrations, migrations, framework)?;
    Ok(BusinessInitializationSql {
        setup,
        migrations: statements,
        verification,
        finish: vec![
            "COMMIT".into(),
            "DROP TEMPORARY TABLE ryframe_schema_assertion_guard".into(),
            "SET SESSION cte_max_recursion_depth=@ryframe_previous_cte_depth".into(),
        ],
    })
}

fn ledger_insert(migration: &dyn BusinessMigration) -> AppResult<String> {
    let before = migration
        .schema()
        .before_fingerprint
        .map(sql_text)
        .unwrap_or_else(|| "NULL".into());
    let tables = serde_json::to_string(&super::ledger::table_names(migration))
        .map_err(|error| AppError::Config(format!("业务迁移表归属序列化失败：{error}")))?;
    Ok(format!(
        "INSERT INTO ryframe_business_migration (scope,module_name,migration_id,checksum,before_fingerprint,after_fingerprint,origin,tables_json,adoption_hash) VALUES ({}, {}, {}, {}, {}, {}, 'executed', {}, NULL)",
        sql_text(migration.scope().as_str()),
        sql_text(migration.module()),
        sql_text(migration.id()),
        sql_text(migration.checksum()),
        before,
        sql_text(migration.schema().after_fingerprint),
        sql_text(&tables)
    ))
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

// 在转义形式编码前移除可吞掉已存在表的建表选项；不改写列默认值或注释正文。
fn strict_create(statement: &str) -> String {
    let mut rest = statement.trim_start();
    for word in ["CREATE", "TABLE", "IF", "NOT", "EXISTS"] {
        let end = rest
            .find(|value: char| !value.is_ascii_alphabetic())
            .unwrap_or(rest.len());
        if !rest[..end].eq_ignore_ascii_case(word) {
            return statement.into();
        }
        rest = rest[end..].trim_start();
    }
    format!("CREATE TABLE {rest}")
}
