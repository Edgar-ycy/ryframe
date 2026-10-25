use ryframe_db::schema::SchemaAssertion;
use ryframe_kernel::{AppError, AppResult};
use ryframe_sdk::BusinessMigration;

/// 导出同一物理控制库的控制面及 shared-control 数据面，不连接数据库。
pub fn shared_control_initialization_sql(
    hashes: &ryframe_db::migration::BootstrapHashes,
    migrations: &[&dyn BusinessMigration],
) -> AppResult<String> {
    let tenant = ryframe_tenant_db::migration::shared_control_initialization().map_err(config)?;
    let ddl = tenant
        .statements
        .iter()
        .filter(|sql| sql.starts_with("CREATE TABLE"))
        .map(String::as_str)
        .collect::<Vec<_>>();
    let framework =
        ryframe_db::migration::framework_initialization_assertions(&ddl).map_err(config)?;
    let business = ryframe_sdk::business_initialization_statements(migrations, &framework)?;
    let mut statements = tenant.statements;
    statements.push(tenant.ledger);
    statements.extend(business.setup);
    statements.extend(business.migrations);
    statements.extend(business.verification);
    // 所有 DDL 已完成，保留最终登记事务直至两侧框架账本完成，不跨 DDL 声称回滚。
    let locks = ryframe_db::schema::schema_registration_locks([
        "seaql_migrations",
        "seaql_tenant_data_migrations",
    ])
    .map_err(config)?;
    statements.extend(locks.tables);
    statements.extend(final_table_set(&framework, migrations));
    statements.extend(SchemaAssertion { category: "framework_ledger", sql: "(SELECT COUNT(*) FROM seaql_migrations)=0 AND (SELECT COUNT(*) FROM seaql_tenant_data_migrations)=0 AND (SELECT COUNT(*) FROM ryframe_initialization_guard WHERE valid=1)=1".into() }.diagnostic_guard("framework", "before-completion"));
    statements.extend(tenant.completion);
    Ok(
        ryframe_db::migration::mysql_initialization_sql_with_business(
            hashes,
            &statements,
            &business.finish,
        ),
    )
}

fn final_table_set(
    framework: &[(String, ryframe_db::schema::SchemaAssertions)],
    migrations: &[&dyn BusinessMigration],
) -> Vec<String> {
    let mut names = framework
        .iter()
        .map(|(table, _)| table.clone())
        .collect::<std::collections::BTreeSet<_>>();
    names.extend([
        "seaql_migrations".to_owned(),
        "seaql_tenant_data_migrations".to_owned(),
    ]);
    if !migrations.is_empty() {
        names.insert("ryframe_business_migration".to_owned());
    }
    for migration in migrations {
        names.extend(
            migration
                .schema()
                .tables
                .iter()
                .map(|table| table.table.to_owned()),
        );
    }
    let known = names
        .iter()
        .map(|name| {
            format!(
                "CAST(CONVERT(X'{}' USING utf8mb4) AS BINARY)",
                name.as_bytes()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    SchemaAssertion { category: "table_set", sql: format!("(SELECT COUNT(*) FROM information_schema.tables WHERE table_schema=DATABASE())={} AND (SELECT COUNT(*) FROM information_schema.tables WHERE table_schema=DATABASE() AND CAST(table_name AS BINARY) IN ({known}))={}", names.len(), names.len()) }.diagnostic_guard("framework-and-business", "final")
}

fn config(error: impl std::fmt::Display) -> AppError {
    AppError::Config(error.to_string())
}
