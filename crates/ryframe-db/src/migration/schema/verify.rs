use sea_orm::{ConnectionTrait, DbBackend, DbErr};

use super::{
    expected::expected_schema,
    inspect::{actual_checks, actual_columns, actual_foreign_keys, actual_indexes, actual_tables},
    normalize::{compatible_column_type, nullable_label},
};

/// 校验完整的规范 MySQL 指纹。
///
/// 该指纹有意覆盖表引擎、字符集、排序规则，列的类型、可空性、默认值、EXTRA、
/// 字符集、排序规则、生成表达式、有序索引、具名外键操作和具名 CHECK 约束。
/// 额外的应用表和规范表上的额外对象均会被拒绝。
pub async fn verify_current_schema<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if db.get_database_backend() != DbBackend::MySql {
        return Err(DbErr::Custom("RyFrame v0.5 only supports MySQL".into()));
    }

    let expected = expected_schema()?;
    let actual_tables = actual_tables(db)
        .await
        .map_err(|error| DbErr::Custom(format!("cannot inspect MySQL tables: {error}")))?;
    let actual_columns = actual_columns(db)
        .await
        .map_err(|error| DbErr::Custom(format!("cannot inspect MySQL columns: {error}")))?;
    let actual_indexes = actual_indexes(db)
        .await
        .map_err(|error| DbErr::Custom(format!("cannot inspect MySQL indexes: {error}")))?;
    let actual_foreign_keys = actual_foreign_keys(db)
        .await
        .map_err(|error| DbErr::Custom(format!("cannot inspect MySQL foreign keys: {error}")))?;
    let actual_checks = actual_checks(db).await.map_err(|error| {
        DbErr::Custom(format!("cannot inspect MySQL check constraints: {error}"))
    })?;
    let mut problems = Vec::new();

    verify_tables(&expected, &actual_tables, &mut problems);
    verify_columns(&expected, &actual_tables, &actual_columns, &mut problems);
    verify_indexes(&expected, &actual_tables, &actual_indexes, &mut problems);
    verify_foreign_keys(
        &expected,
        &actual_tables,
        &actual_foreign_keys,
        &mut problems,
    );
    verify_checks(&expected, &actual_tables, &actual_checks, &mut problems);
    schema_problems(problems)
}

fn verify_tables(
    expected: &super::types::ExpectedSchema,
    actual_tables: &std::collections::BTreeMap<String, super::types::ActualTable>,
    problems: &mut Vec<String>,
) {
    for (table, expected_table) in &expected.tables {
        let Some(actual) = actual_tables.get(table) else {
            problems.push(format!("missing table {table}"));
            continue;
        };
        if actual.engine != expected_table.engine {
            problems.push(format!(
                "table {table} uses engine {}, expected {}",
                actual.engine, expected_table.engine
            ));
        }
        if actual.character_set != expected_table.character_set {
            problems.push(format!(
                "table {table} has character set {}, expected {}",
                actual.character_set, expected_table.character_set
            ));
        }
        if actual.collation != expected_table.collation {
            problems.push(format!(
                "table {table} has collation {}, expected {}",
                actual.collation, expected_table.collation
            ));
        }
    }
    for table in actual_tables.keys() {
        if table != "seaql_migrations" && !expected.tables.contains_key(table) {
            problems.push(format!("unexpected application table {table}"));
        }
    }
}

fn verify_columns(
    expected: &super::types::ExpectedSchema,
    actual_tables: &std::collections::BTreeMap<String, super::types::ActualTable>,
    actual_columns: &std::collections::BTreeMap<(String, String), super::types::ActualColumn>,
    problems: &mut Vec<String>,
) {
    for ((table, column), expected_column) in &expected.columns {
        if !actual_tables.contains_key(table) {
            continue;
        }
        let Some(actual) = actual_columns.get(&(table.clone(), column.clone())) else {
            problems.push(format!("missing column {table}.{column}"));
            continue;
        };
        if !compatible_column_type(&expected_column.column_type, &actual.column_type) {
            problems.push(format!(
                "column {table}.{column} has type {}, expected {}",
                actual.column_type, expected_column.column_type
            ));
        }
        if expected_column.nullable != actual.nullable {
            problems.push(format!(
                "column {table}.{column} nullability is {}, expected {}",
                nullable_label(actual.nullable),
                nullable_label(expected_column.nullable)
            ));
        }
        if expected_column.default != actual.default {
            problems.push(format!(
                "column {table}.{column} has default {:?}, expected {:?}",
                actual.default, expected_column.default
            ));
        }
        if expected_column.extra != actual.extra {
            problems.push(format!(
                "column {table}.{column} has EXTRA {:?}, expected {:?}",
                actual.extra, expected_column.extra
            ));
        }
        if expected_column.character_set != actual.character_set {
            problems.push(format!(
                "column {table}.{column} has character set {:?}, expected {:?}",
                actual.character_set, expected_column.character_set
            ));
        }
        if expected_column.collation != actual.collation {
            problems.push(format!(
                "column {table}.{column} has collation {:?}, expected {:?}",
                actual.collation, expected_column.collation
            ));
        }
        if expected_column.generation_expression != actual.generation_expression {
            problems.push(format!(
                "column {table}.{column} has generation expression {:?}, expected {:?}",
                actual.generation_expression, expected_column.generation_expression
            ));
        }
    }
    for (table, column) in actual_columns.keys() {
        if expected.tables.contains_key(table)
            && !expected
                .columns
                .contains_key(&(table.clone(), column.clone()))
        {
            problems.push(format!("unexpected column {table}.{column}"));
        }
    }
}

fn verify_indexes(
    expected: &super::types::ExpectedSchema,
    actual_tables: &std::collections::BTreeMap<String, super::types::ActualTable>,
    actual_indexes: &std::collections::BTreeMap<(String, String), super::types::ActualIndex>,
    problems: &mut Vec<String>,
) {
    for ((table, name), expected_index) in &expected.indexes {
        if !actual_tables.contains_key(table) {
            continue;
        }
        let Some(actual) = actual_indexes.get(&(table.clone(), name.clone())) else {
            problems.push(format!("missing index {table}.{name}"));
            continue;
        };
        let (actual_columns, has_prefix) = ordered_index_columns(&actual.columns);
        if actual.unique != expected_index.unique
            || actual.index_type != "btree"
            || !actual.visible
            || has_prefix
            || actual_columns != expected_index.columns
        {
            problems.push(format!(
                "index {table}.{name} does not match canonical definition"
            ));
        }
    }
    for table_and_name in actual_indexes.keys() {
        let (table, name) = table_and_name;
        if expected.tables.contains_key(table) && !expected.indexes.contains_key(table_and_name) {
            problems.push(format!("unexpected index {table}.{name}"));
        }
    }
}

fn verify_foreign_keys(
    expected: &super::types::ExpectedSchema,
    actual_tables: &std::collections::BTreeMap<String, super::types::ActualTable>,
    actual_foreign_keys: &std::collections::BTreeMap<
        (String, String),
        super::types::ActualForeignKey,
    >,
    problems: &mut Vec<String>,
) {
    for ((table, name), expected_foreign_key) in &expected.foreign_keys {
        if !actual_tables.contains_key(table) {
            continue;
        }
        let Some(actual) = actual_foreign_keys.get(&(table.clone(), name.clone())) else {
            problems.push(format!("missing foreign key {table}.{name}"));
            continue;
        };
        let actual_columns = ordered_columns(&actual.columns);
        let referenced_columns = ordered_columns(&actual.referenced_columns);
        if actual_columns != expected_foreign_key.columns
            || actual.referenced_table != expected_foreign_key.referenced_table
            || referenced_columns != expected_foreign_key.referenced_columns
            || actual.update_rule != expected_foreign_key.update_rule
            || actual.delete_rule != expected_foreign_key.delete_rule
        {
            problems.push(format!(
                "foreign key {table}.{name} does not match canonical definition"
            ));
        }
    }
    for table_and_name in actual_foreign_keys.keys() {
        let (table, name) = table_and_name;
        if expected.tables.contains_key(table)
            && !expected.foreign_keys.contains_key(table_and_name)
        {
            problems.push(format!("unexpected foreign key {table}.{name}"));
        }
    }
}

fn verify_checks(
    expected: &super::types::ExpectedSchema,
    actual_tables: &std::collections::BTreeMap<String, super::types::ActualTable>,
    actual_checks: &std::collections::BTreeMap<(String, String), super::types::ActualCheck>,
    problems: &mut Vec<String>,
) {
    for ((table, name), expected_check) in &expected.checks {
        if !actual_tables.contains_key(table) {
            continue;
        }
        let Some(actual) = actual_checks.get(&(table.clone(), name.clone())) else {
            problems.push(format!("missing check constraint {table}.{name}"));
            continue;
        };
        if actual.clause != expected_check.clause {
            problems.push(format!(
                "check constraint {table}.{name} does not match canonical expression"
            ));
        }
        if actual.enforced != expected_check.enforced {
            problems.push(format!(
                "check constraint {table}.{name} enforcement is {}, expected {}",
                enforcement_label(actual.enforced),
                enforcement_label(expected_check.enforced)
            ));
        }
    }
    for table_and_name in actual_checks.keys() {
        let (table, name) = table_and_name;
        if expected.tables.contains_key(table) && !expected.checks.contains_key(table_and_name) {
            problems.push(format!("unexpected check constraint {table}.{name}"));
        }
    }
}

fn enforcement_label(enforced: bool) -> &'static str {
    if enforced { "ENFORCED" } else { "NOT ENFORCED" }
}

fn ordered_columns(columns: &[(i64, String)]) -> Vec<String> {
    let mut columns = columns.to_vec();
    columns.sort_by_key(|(sequence, _)| *sequence);
    columns.into_iter().map(|(_, column)| column).collect()
}

fn ordered_index_columns(columns: &[(i64, String, Option<i64>)]) -> (Vec<String>, bool) {
    let mut columns = columns.to_vec();
    columns.sort_by_key(|(sequence, _, _)| *sequence);
    let has_prefix = columns.iter().any(|(_, _, prefix)| prefix.is_some());
    (
        columns.into_iter().map(|(_, column, _)| column).collect(),
        has_prefix,
    )
}

fn schema_problems(problems: Vec<String>) -> Result<(), DbErr> {
    if problems.is_empty() {
        return Ok(());
    }
    let total = problems.len();
    let summary = problems.into_iter().take(25).collect::<Vec<_>>().join("; ");
    let suffix = if total > 25 {
        format!("; and {} more", total - 25)
    } else {
        String::new()
    };
    Err(DbErr::Custom(format!(
        "RyFrame schema verification failed ({total} mismatches): {summary}{suffix}"
    )))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::verify_checks;
    use crate::migration::schema::types::{
        ActualCheck, ActualTable, ExpectedCheck, ExpectedSchema, ExpectedTable,
    };

    fn check_problems(actual_checks: BTreeMap<(String, String), ActualCheck>) -> Vec<String> {
        let mut expected = ExpectedSchema::default();
        expected.tables.insert(
            "sys_restore_run".into(),
            ExpectedTable {
                engine: "innodb".into(),
                character_set: "utf8mb4".into(),
                collation: "utf8mb4_general_ci".into(),
            },
        );
        expected.checks.insert(
            ("sys_restore_run".into(), "ck_restore_run_completed".into()),
            ExpectedCheck {
                clause: "terminal_completed_at_contract".into(),
                enforced: true,
            },
        );
        let actual_tables = BTreeMap::from([(
            "sys_restore_run".into(),
            ActualTable {
                engine: "innodb".into(),
                character_set: "utf8mb4".into(),
                collation: "utf8mb4_general_ci".into(),
            },
        )]);
        let mut problems = Vec::new();
        verify_checks(&expected, &actual_tables, &actual_checks, &mut problems);
        problems
    }

    #[test]
    fn check_constraints_require_name_expression_enforcement_and_no_extras() {
        let key = ("sys_restore_run".into(), "ck_restore_run_completed".into());
        let matching = BTreeMap::from([(
            key.clone(),
            ActualCheck {
                clause: "terminal_completed_at_contract".into(),
                enforced: true,
            },
        )]);
        assert!(check_problems(matching).is_empty());

        let changed = BTreeMap::from([(
            key.clone(),
            ActualCheck {
                clause: "weakened_terminal_contract".into(),
                enforced: true,
            },
        )]);
        assert_eq!(
            check_problems(changed),
            [
                "check constraint sys_restore_run.ck_restore_run_completed does not match canonical expression"
            ]
        );

        let disabled = BTreeMap::from([(
            key.clone(),
            ActualCheck {
                clause: "terminal_completed_at_contract".into(),
                enforced: false,
            },
        )]);
        assert_eq!(
            check_problems(disabled),
            [
                "check constraint sys_restore_run.ck_restore_run_completed enforcement is NOT ENFORCED, expected ENFORCED"
            ]
        );

        assert_eq!(
            check_problems(BTreeMap::new()),
            ["missing check constraint sys_restore_run.ck_restore_run_completed"]
        );

        let with_extra = BTreeMap::from([
            (
                key,
                ActualCheck {
                    clause: "terminal_completed_at_contract".into(),
                    enforced: true,
                },
            ),
            (
                ("sys_restore_run".into(), "ck_unexpected".into()),
                ActualCheck {
                    clause: "status='running'".into(),
                    enforced: true,
                },
            ),
        ]);
        assert_eq!(
            check_problems(with_extra),
            ["unexpected check constraint sys_restore_run.ck_unexpected"]
        );
    }
}
