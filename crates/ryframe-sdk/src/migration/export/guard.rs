use super::{BusinessMigration, sql_text};
use ryframe_db::schema::{
    SchemaAssertion, SchemaAssertions, schema_assertions, schema_preconditions,
    schema_registration_locks,
};
use ryframe_kernel::{AppError, AppResult};
use std::collections::BTreeMap;

type Framework = [(String, SchemaAssertions)];

pub(super) fn validate_owners(migrations: &[&dyn BusinessMigration]) -> AppResult<()> {
    let mut owners = BTreeMap::new();
    for migration in migrations {
        for table in migration.schema().tables {
            let owner = (migration.scope(), migration.module());
            if let Some(previous) = owners.insert(table.table, owner)
                && previous != owner
            {
                return Err(AppError::Config(format!(
                    "同一物理初始化库的业务表 {} 归属冲突；未生成初始化 SQL",
                    table.table
                )));
            }
        }
    }
    Ok(())
}

pub(super) fn before(
    migration: &dyn BusinessMigration,
    registered: &[&dyn BusinessMigration],
    framework: &Framework,
) -> AppResult<Vec<String>> {
    let existing = migration
        .schema()
        .tables
        .iter()
        .filter(|table| table.before.is_some())
        .map(|table| table.table);
    let mut sql = locks(existing, framework, true)?;
    sql.extend(framework_guards(framework, "before"));
    sql.extend(ledger(registered)?);
    for table in migration.schema().tables {
        let plan = schema_preconditions(table.table, table.before).map_err(config)?;
        sql.extend(guards(
            &plan,
            &format!("{}/{}:{}", migration.module(), migration.id(), table.table),
            "before",
        ));
    }
    sql.push("COMMIT".into());
    Ok(sql)
}

pub(super) fn after(
    current: &[&dyn BusinessMigration],
    registered: &[&dyn BusinessMigration],
    framework: &Framework,
) -> AppResult<Vec<String>> {
    let latest = latest(current);
    let mut sql = locks(latest.keys().copied(), framework, !current.is_empty())?;
    sql.extend(framework_guards(framework, "final"));
    for (table, migration) in latest {
        let definition = migration
            .schema()
            .tables
            .iter()
            .find(|t| t.table == table)
            .expect("latest 来自当前声明");
        let plan = schema_assertions(table, definition.after).map_err(config)?;
        sql.extend(guards(
            &plan,
            &format!("{}/{}:{table}", migration.module(), migration.id()),
            "after",
        ));
    }
    if !current.is_empty() {
        sql.extend(ledger(registered)?);
    }
    Ok(sql)
}

fn latest<'a>(
    migrations: &[&'a dyn BusinessMigration],
) -> BTreeMap<&'static str, &'a dyn BusinessMigration> {
    let mut latest = BTreeMap::new();
    for migration in migrations {
        for table in migration.schema().tables {
            latest.insert(table.table, *migration);
        }
    }
    latest
}

fn locks(
    tables: impl IntoIterator<Item = &'static str>,
    framework: &Framework,
    ledger: bool,
) -> AppResult<Vec<String>> {
    let mut names = tables.into_iter().map(str::to_owned).collect::<Vec<_>>();
    names.extend(framework.iter().map(|(table, _)| table.clone()));
    if ledger {
        names.push(super::super::ledger::NAME.into());
    }
    let phase = schema_registration_locks(names.iter().map(String::as_str)).map_err(config)?;
    Ok(phase
        .begin
        .into_iter()
        .map(str::to_owned)
        .chain(phase.tables)
        .collect())
}

fn framework_guards(framework: &Framework, phase: &str) -> Vec<String> {
    framework
        .iter()
        .flat_map(|(table, plan)| guards(plan, &format!("framework:{table}"), phase))
        .collect()
}

fn guards(plan: &SchemaAssertions, identity: &str, phase: &str) -> Vec<String> {
    plan.predicates
        .iter()
        .flat_map(|p| p.diagnostic_guard(identity, phase))
        .collect()
}

fn ledger(migrations: &[&dyn BusinessMigration]) -> AppResult<Vec<String>> {
    let mut predicates = vec![format!(
        "(SELECT COUNT(*) FROM ryframe_business_migration)={}",
        migrations.len()
    )];
    for migration in migrations {
        let before = migration
            .schema()
            .before_fingerprint
            .map(sql_text)
            .unwrap_or_else(|| "NULL".into());
        let tables = serde_json::to_string(&super::super::ledger::table_names(*migration))
            .map_err(|e| AppError::Config(format!("业务表归属序列化失败：{e}")))?;
        predicates.push(format!("(SELECT COUNT(*) FROM ryframe_business_migration WHERE CAST(scope AS BINARY)=CAST({} AS BINARY) AND CAST(module_name AS BINARY)=CAST({} AS BINARY) AND CAST(migration_id AS BINARY)=CAST({} AS BINARY) AND CAST(checksum AS BINARY)=CAST({} AS BINARY) AND CAST(before_fingerprint AS BINARY)<=>CAST({before} AS BINARY) AND CAST(after_fingerprint AS BINARY)=CAST({} AS BINARY) AND CAST(origin AS BINARY)=CAST('executed' AS BINARY) AND adoption_hash IS NULL AND CAST(tables_json AS BINARY)=CAST(CAST({} AS JSON) AS BINARY))=1", sql_text(migration.scope().as_str()), sql_text(migration.module()), sql_text(migration.id()), sql_text(migration.checksum()), sql_text(migration.schema().after_fingerprint), sql_text(&tables)));
    }
    Ok(SchemaAssertion {
        category: "ledger_ownership",
        sql: predicates.join(" AND "),
    }
    .diagnostic_guard("business-migrations", "ledger"))
}

fn config(error: impl std::fmt::Display) -> AppError {
    AppError::Config(error.to_string())
}
