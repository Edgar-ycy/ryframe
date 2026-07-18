use std::collections::BTreeMap;

use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement, TryGetable};

#[cfg(test)]
use crate::m20260522_000000_mysql_baseline::REQUIRED_TABLES;
use crate::m20260522_000000_mysql_baseline::ddl_statements;

#[derive(Debug, PartialEq, Eq)]
struct ExpectedTable {
    engine: String,
    character_set: String,
    collation: String,
}

#[derive(Debug, PartialEq, Eq)]
struct ExpectedColumn {
    column_type: String,
    nullable: bool,
    default: Option<String>,
    extra: String,
    character_set: Option<String>,
    collation: Option<String>,
    generation_expression: String,
}

#[derive(Debug, PartialEq, Eq)]
struct ExpectedIndex {
    unique: bool,
    columns: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct ExpectedForeignKey {
    columns: Vec<String>,
    referenced_table: String,
    referenced_columns: Vec<String>,
    update_rule: String,
    delete_rule: String,
}

#[derive(Default)]
struct ExpectedSchema {
    tables: BTreeMap<String, ExpectedTable>,
    columns: BTreeMap<(String, String), ExpectedColumn>,
    indexes: BTreeMap<(String, String), ExpectedIndex>,
    foreign_keys: BTreeMap<(String, String), ExpectedForeignKey>,
}

#[derive(Debug)]
struct ActualTable {
    engine: String,
    character_set: String,
    collation: String,
}

#[derive(Debug)]
struct ActualColumn {
    column_type: String,
    nullable: bool,
    default: Option<String>,
    extra: String,
    character_set: Option<String>,
    collation: Option<String>,
    generation_expression: String,
}

#[derive(Default)]
struct ActualIndex {
    unique: bool,
    index_type: String,
    visible: bool,
    columns: Vec<(i64, String, Option<i64>)>,
}

#[derive(Default)]
struct ActualForeignKey {
    columns: Vec<(i64, String)>,
    referenced_table: String,
    referenced_columns: Vec<(i64, String)>,
    update_rule: String,
    delete_rule: String,
}

pub(crate) async fn user_tables<C>(db: &C) -> Result<Vec<String>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT TABLE_NAME FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE' \
             AND TABLE_NAME <> 'seaql_migrations' ORDER BY TABLE_NAME"
                .to_owned(),
        ))
        .await?;
    let mut tables = Vec::with_capacity(rows.len());
    for row in rows {
        tables.push(String::try_get_by_index(&row, 0)?);
    }
    Ok(tables)
}

pub(crate) async fn verify_legacy_schema<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    verify_schema(db, true).await
}

/// Verify the complete canonical MySQL fingerprint.
///
/// The fingerprint deliberately covers table engine/charset/collation, column
/// type/nullability/default/EXTRA/charset/collation/generation expression,
/// ordered indexes, and named foreign-key actions.
/// Extra application tables and extra objects on canonical tables are rejected.
pub async fn verify_current_schema<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    verify_schema(db, false).await
}

async fn verify_schema<C>(db: &C, legacy: bool) -> Result<(), DbErr>
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
    let mut problems = Vec::new();

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

    for ((table, column), expected_column) in &expected.columns {
        if !actual_tables.contains_key(table) || (legacy && is_upgrade_column(table, column)) {
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

    for ((table, name), expected_index) in &expected.indexes {
        if !actual_tables.contains_key(table)
            || (legacy
                && (expected_index
                    .columns
                    .iter()
                    .any(|column| is_upgrade_column(table, column))
                    || is_upgrade_index(table, name)))
        {
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
        if expected.tables.contains_key(table)
            && !expected.indexes.contains_key(table_and_name)
            && !(legacy && is_legacy_extra_index(table, name))
        {
            problems.push(format!("unexpected index {table}.{name}"));
        }
    }

    for ((table, name), expected_foreign_key) in &expected.foreign_keys {
        if !actual_tables.contains_key(table)
            || (legacy
                && (expected_foreign_key
                    .columns
                    .iter()
                    .any(|column| is_upgrade_column(table, column))
                    || is_upgrade_foreign_key(table, name)))
        {
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
            && !(legacy && is_legacy_extra_foreign_key(table, name))
        {
            problems.push(format!("unexpected foreign key {table}.{name}"));
        }
    }

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

async fn actual_tables<C>(db: &C) -> Result<BTreeMap<String, ActualTable>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT t.TABLE_NAME, t.ENGINE, c.CHARACTER_SET_NAME, t.TABLE_COLLATION \
             FROM information_schema.TABLES t \
             JOIN information_schema.COLLATION_CHARACTER_SET_APPLICABILITY c \
               ON c.COLLATION_NAME = t.TABLE_COLLATION \
             WHERE t.TABLE_SCHEMA = DATABASE() AND t.TABLE_TYPE = 'BASE TABLE'"
                .to_owned(),
        ))
        .await?;
    let mut tables = BTreeMap::new();
    for row in rows {
        let table = String::try_get_by_index(&row, 0)?;
        let engine = normalize_identifier(&String::try_get_by_index(&row, 1)?);
        let character_set = normalize_identifier(&String::try_get_by_index(&row, 2)?);
        let collation = normalize_identifier(&String::try_get_by_index(&row, 3)?);
        tables.insert(
            table,
            ActualTable {
                engine,
                character_set,
                collation,
            },
        );
    }
    Ok(tables)
}

async fn actual_columns<C>(db: &C) -> Result<BTreeMap<(String, String), ActualColumn>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, EXTRA, \
                    CHARACTER_SET_NAME, COLLATION_NAME, GENERATION_EXPRESSION \
             FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = DATABASE()"
                .to_owned(),
        ))
        .await?;
    let mut columns = BTreeMap::new();
    for row in rows {
        let table = String::try_get_by_index(&row, 0)?;
        let column = String::try_get_by_index(&row, 1)?;
        let column_type = String::try_get_by_index(&row, 2)?;
        let nullable = String::try_get_by_index(&row, 3)? == "YES";
        let default =
            Option::<String>::try_get_by_index(&row, 4)?.map(|value| normalize_default(&value));
        let extra = normalize_actual_extra(&String::try_get_by_index(&row, 5)?);
        let character_set =
            Option::<String>::try_get_by_index(&row, 6)?.map(|value| normalize_identifier(&value));
        let collation =
            Option::<String>::try_get_by_index(&row, 7)?.map(|value| normalize_identifier(&value));
        let generation_expression =
            normalize_generation_expression(&String::try_get_by_index(&row, 8)?);
        columns.insert(
            (table, column),
            ActualColumn {
                column_type: normalize_column_type(&column_type),
                nullable,
                default,
                extra,
                character_set,
                collation,
                generation_expression,
            },
        );
    }
    Ok(columns)
}

async fn actual_indexes<C>(db: &C) -> Result<BTreeMap<(String, String), ActualIndex>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT TABLE_NAME, INDEX_NAME, CAST(NON_UNIQUE AS SIGNED), \
                    CAST(SEQ_IN_INDEX AS SIGNED), COALESCE(COLUMN_NAME, EXPRESSION), \
                    CAST(SUB_PART AS SIGNED), INDEX_TYPE, IS_VISIBLE \
             FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = DATABASE() \
             ORDER BY TABLE_NAME, INDEX_NAME, SEQ_IN_INDEX"
                .to_owned(),
        ))
        .await?;
    let mut indexes = BTreeMap::<(String, String), ActualIndex>::new();
    for row in rows {
        let table = String::try_get_by_index(&row, 0)?;
        let name = String::try_get_by_index(&row, 1)?;
        let non_unique = i64::try_get_by_index(&row, 2)?;
        let sequence = i64::try_get_by_index(&row, 3)?;
        let column = String::try_get_by_index(&row, 4)?;
        let prefix_length = Option::<i64>::try_get_by_index(&row, 5)?;
        let index_type = normalize_identifier(&String::try_get_by_index(&row, 6)?);
        let visible = String::try_get_by_index(&row, 7)? == "YES";
        let entry = indexes.entry((table, name)).or_default();
        entry.unique = non_unique == 0;
        entry.index_type = index_type;
        entry.visible = visible;
        entry.columns.push((sequence, column, prefix_length));
    }
    Ok(indexes)
}

async fn actual_foreign_keys<C>(
    db: &C,
) -> Result<BTreeMap<(String, String), ActualForeignKey>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT k.TABLE_NAME, k.CONSTRAINT_NAME, \
                    CAST(k.ORDINAL_POSITION AS SIGNED), k.COLUMN_NAME, \
                    k.REFERENCED_TABLE_NAME, k.REFERENCED_COLUMN_NAME, \
                    r.UPDATE_RULE, r.DELETE_RULE \
             FROM information_schema.KEY_COLUMN_USAGE k \
             JOIN information_schema.REFERENTIAL_CONSTRAINTS r \
               ON r.CONSTRAINT_SCHEMA = k.CONSTRAINT_SCHEMA \
              AND r.TABLE_NAME = k.TABLE_NAME \
              AND r.CONSTRAINT_NAME = k.CONSTRAINT_NAME \
             WHERE k.CONSTRAINT_SCHEMA = DATABASE() \
               AND k.REFERENCED_TABLE_NAME IS NOT NULL \
             ORDER BY k.TABLE_NAME, k.CONSTRAINT_NAME, k.ORDINAL_POSITION"
                .to_owned(),
        ))
        .await?;
    let mut foreign_keys = BTreeMap::<(String, String), ActualForeignKey>::new();
    for row in rows {
        let table = String::try_get_by_index(&row, 0)?;
        let name = String::try_get_by_index(&row, 1)?;
        let sequence = i64::try_get_by_index(&row, 2)?;
        let column = String::try_get_by_index(&row, 3)?;
        let referenced_table = String::try_get_by_index(&row, 4)?;
        let referenced_column = String::try_get_by_index(&row, 5)?;
        let update_rule = normalize_action(&String::try_get_by_index(&row, 6)?);
        let delete_rule = normalize_action(&String::try_get_by_index(&row, 7)?);
        let entry = foreign_keys.entry((table, name)).or_default();
        entry.referenced_table = referenced_table;
        entry.update_rule = update_rule;
        entry.delete_rule = delete_rule;
        entry.columns.push((sequence, column));
        entry.referenced_columns.push((sequence, referenced_column));
    }
    Ok(foreign_keys)
}

fn expected_schema() -> Result<ExpectedSchema, DbErr> {
    let mut schema = ExpectedSchema::default();
    for statement in ddl_statements() {
        let table = extract_table_name(statement).ok_or_else(|| {
            DbErr::Custom("canonical baseline contains an invalid CREATE TABLE statement".into())
        })?;
        let engine = extract_ddl_option(statement, "ENGINE=")?;
        let table_character_set = extract_ddl_option(statement, "DEFAULT CHARSET=")?;
        let table_collation = extract_ddl_option(statement, "COLLATE=")?;
        schema.tables.insert(
            table.clone(),
            ExpectedTable {
                engine,
                character_set: table_character_set.clone(),
                collation: table_collation.clone(),
            },
        );

        let lines = statement.lines().collect::<Vec<_>>();
        let mut pending_constraint = None;
        let mut index = 0;
        while index < lines.len() {
            let line = lines[index].trim().trim_end_matches(',');
            let upper = line.to_ascii_uppercase();
            if line.starts_with('`') {
                let identifiers = backtick_identifiers(line);
                let Some(column) = identifiers.first() else {
                    index += 1;
                    continue;
                };
                let after_name = line
                    .split_once('`')
                    .and_then(|(_, rest)| rest.split_once('`'))
                    .map(|(_, rest)| rest.trim_start())
                    .unwrap_or_default();
                let column_type = normalize_column_type(extract_column_type(after_name));
                let uses_character_set = column_type_uses_character_set(&column_type);
                schema.columns.insert(
                    (table.clone(), column.clone()),
                    ExpectedColumn {
                        column_type,
                        nullable: !upper.contains("NOT NULL"),
                        default: extract_column_default(after_name),
                        extra: expected_extra(after_name),
                        character_set: uses_character_set.then(|| {
                            extract_identifier_option(after_name, "CHARACTER SET")
                                .unwrap_or_else(|| table_character_set.clone())
                        }),
                        collation: uses_character_set.then(|| {
                            extract_identifier_option(after_name, "COLLATE")
                                .unwrap_or_else(|| table_collation.clone())
                        }),
                        generation_expression: expected_generation_expression(after_name),
                    },
                );
            } else if upper.starts_with("PRIMARY KEY") {
                schema.indexes.insert(
                    (table.clone(), "PRIMARY".into()),
                    ExpectedIndex {
                        unique: true,
                        columns: backtick_identifiers(line),
                    },
                );
            } else if upper.starts_with("UNIQUE KEY") || upper.starts_with("KEY ") {
                let identifiers = backtick_identifiers(line);
                if let Some((name, columns)) = identifiers.split_first() {
                    schema.indexes.insert(
                        (table.clone(), name.clone()),
                        ExpectedIndex {
                            unique: upper.starts_with("UNIQUE KEY"),
                            columns: columns.to_vec(),
                        },
                    );
                }
            } else if upper.starts_with("CONSTRAINT ") {
                pending_constraint = backtick_identifiers(line).into_iter().next();
            } else if upper.starts_with("FOREIGN KEY") {
                let Some(name) = pending_constraint.take() else {
                    return Err(DbErr::Custom(format!(
                        "foreign key in {table} is missing a constraint name"
                    )));
                };
                let mut clause = line.to_owned();
                while index + 1 < lines.len() {
                    let next = lines[index + 1].trim().trim_end_matches(',');
                    if next.is_empty() {
                        index += 1;
                        continue;
                    }
                    if !next.to_ascii_uppercase().starts_with("ON ") {
                        break;
                    }
                    index += 1;
                    clause.push(' ');
                    clause.push_str(next);
                }
                let clause_upper = clause.to_ascii_uppercase();
                let Some(reference_at) = clause_upper.find("REFERENCES") else {
                    return Err(DbErr::Custom(format!(
                        "foreign key {table}.{name} is missing REFERENCES"
                    )));
                };
                let columns = backtick_identifiers(&clause[..reference_at]);
                let referenced = backtick_identifiers(&clause[reference_at..]);
                let Some((referenced_table, referenced_columns)) = referenced.split_first() else {
                    return Err(DbErr::Custom(format!(
                        "foreign key {table}.{name} has an invalid target"
                    )));
                };
                schema.foreign_keys.insert(
                    (table.clone(), name),
                    ExpectedForeignKey {
                        columns,
                        referenced_table: referenced_table.clone(),
                        referenced_columns: referenced_columns.to_vec(),
                        update_rule: extract_action(&clause, "ON UPDATE")
                            .unwrap_or_else(|| "restrict".into()),
                        delete_rule: extract_action(&clause, "ON DELETE")
                            .unwrap_or_else(|| "restrict".into()),
                    },
                );
            }
            index += 1;
        }
    }
    Ok(schema)
}

fn extract_table_name(statement: &str) -> Option<String> {
    backtick_identifiers(statement).into_iter().next()
}

fn extract_ddl_option(statement: &str, option: &str) -> Result<String, DbErr> {
    let upper = statement.to_ascii_uppercase();
    let start = upper.find(option).ok_or_else(|| {
        DbErr::Custom(format!(
            "canonical baseline is missing table option {option}"
        ))
    })? + option.len();
    let value = statement[start..]
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_end_matches(';');
    if value.is_empty() {
        return Err(DbErr::Custom(format!(
            "canonical baseline has an empty table option {option}"
        )));
    }
    Ok(normalize_identifier(value))
}

fn extract_column_type(value: &str) -> &str {
    let value = value.trim_start();
    let first_whitespace = value.find(char::is_whitespace).unwrap_or(value.len());
    if let Some(open) = value[..first_whitespace].find('(')
        && let Some(close) = value[open + 1..].find(')')
    {
        return &value[..open + close + 2];
    }
    &value[..first_whitespace]
}

fn extract_identifier_option(value: &str, keyword: &str) -> Option<String> {
    let upper = value.to_ascii_uppercase();
    let start = upper.find(keyword)? + keyword.len();
    let identifier = value[start..].split_whitespace().next()?.trim_matches('`');
    (!identifier.is_empty()).then(|| normalize_identifier(identifier))
}

fn column_type_uses_character_set(column_type: &str) -> bool {
    let base = column_type
        .split_once('(')
        .map_or(column_type, |(base, _)| base);
    matches!(
        base,
        "char" | "varchar" | "tinytext" | "text" | "mediumtext" | "longtext" | "enum" | "set"
    )
}

fn expected_generation_expression(value: &str) -> String {
    let upper = value.to_ascii_uppercase();
    if !upper.contains(" GENERATED") && !upper.contains(" AS (") {
        return String::new();
    }
    let Some(start) = upper.find(" AS (").map(|index| index + " AS (".len()) else {
        return String::new();
    };
    let Some(end) = value[start..].rfind(')') else {
        return String::new();
    };
    normalize_generation_expression(&value[start..start + end])
}

fn extract_column_default(value: &str) -> Option<String> {
    let upper = value.to_ascii_uppercase();
    let search_end = upper.find(" COMMENT ").unwrap_or(value.len());
    let before_comment = &value[..search_end];
    let before_comment_upper = &upper[..search_end];
    let start = before_comment_upper.find("DEFAULT")? + "DEFAULT".len();
    let raw = before_comment[start..].trim_start();
    if raw.to_ascii_uppercase().starts_with("NULL") {
        return None;
    }
    if let Some(raw) = raw.strip_prefix('\'') {
        let mut output = String::new();
        let mut characters = raw.chars().peekable();
        while let Some(character) = characters.next() {
            if character == '\'' {
                if characters.peek() == Some(&'\'') {
                    output.push('\'');
                    characters.next();
                    continue;
                }
                break;
            }
            output.push(character);
        }
        return Some(normalize_default(&output));
    }
    Some(normalize_default(
        raw.split_whitespace().next().unwrap_or_default(),
    ))
}

fn extract_action(clause: &str, keyword: &str) -> Option<String> {
    let upper = clause.to_ascii_uppercase();
    let start = upper.find(keyword)? + keyword.len();
    let action = clause[start..]
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>();
    let action = if action.first()?.eq_ignore_ascii_case("SET")
        || action.first()?.eq_ignore_ascii_case("NO")
    {
        action.join(" ")
    } else {
        action[0].to_owned()
    };
    Some(normalize_action(&action))
}

fn backtick_identifiers(value: &str) -> Vec<String> {
    value
        .split('`')
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, identifier)| identifier.to_owned())
        .collect()
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

fn normalize_identifier(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn normalize_column_type(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect()
}

fn normalize_default(value: &str) -> String {
    let value = value.trim();
    if value.eq_ignore_ascii_case("current_timestamp")
        || value.eq_ignore_ascii_case("current_timestamp()")
    {
        "current_timestamp".into()
    } else {
        value.to_owned()
    }
}

fn expected_extra(value: &str) -> String {
    let lower = value
        .to_ascii_lowercase()
        .replace("current_timestamp()", "current_timestamp");
    let mut parts = Vec::new();
    if lower.contains("auto_increment") {
        parts.push("auto_increment");
    }
    if lower.contains("on update current_timestamp") {
        parts.push("on update current_timestamp");
    }
    parts.join(" ")
}

fn normalize_actual_extra(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .replace("current_timestamp()", "current_timestamp")
        .split_whitespace()
        .filter(|part| *part != "default_generated")
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_generation_expression(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|character| !character.is_ascii_whitespace() && *character != '`')
        .collect()
}

fn normalize_action(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    if value == "no action" {
        "restrict".into()
    } else {
        value
    }
}

fn compatible_column_type(expected: &str, actual: &str) -> bool {
    expected == actual
        || matches!(
            (expected, actual),
            ("tinyint(1)", "tinyint") | ("tinyint", "tinyint(1)")
        )
}

fn nullable_label(nullable: bool) -> &'static str {
    if nullable { "NULL" } else { "NOT NULL" }
}

// These are the only canonical v0.5 objects that a pre-v0.5 RyFrame schema may
// omit. Any other drift is treated as a partial or incompatible initialization.
fn is_upgrade_column(table: &str, column: &str) -> bool {
    matches!(
        (table, column),
        ("sys_tenant", "session_version")
            | ("sys_user", "auth_version")
            | ("sys_role", "is_super")
            | ("sys_menu", "perm_id" | "route_key")
            | (
                "sys_login_info" | "sys_oper_log" | "password_reset_requests",
                "tenant_id"
            )
    )
}

fn is_upgrade_index(table: &str, name: &str) -> bool {
    matches!(
        (table, name),
        (
            "sys_menu",
            "idx_perm_id" | "idx_menu_tenant_perm" | "idx_menu_tenant_route"
        ) | ("sys_login_info" | "sys_oper_log", "idx_tenant_id")
            | ("password_reset_requests", "idx_password_reset_tenant")
    )
}

fn is_upgrade_foreign_key(table: &str, name: &str) -> bool {
    matches!(
        (table, name),
        ("sys_menu", "fk_sys_menu_permission")
            | ("sys_login_info", "fk_sys_login_info_tenant")
            | ("sys_oper_log", "fk_sys_oper_log_tenant")
            | ("password_reset_requests", "fk_password_reset_tenant")
            | (
                "sys_role_permission",
                "fk_sys_role_permission_role" | "fk_sys_role_permission_permission"
            )
            | (
                "sys_role_dept",
                "fk_sys_role_dept_role" | "fk_sys_role_dept_dept"
            )
    )
}

fn is_legacy_extra_index(_table: &str, _name: &str) -> bool {
    false
}

fn is_legacy_extra_foreign_key(_table: &str, _name: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_schema_parser_covers_every_required_table() {
        let schema = expected_schema().unwrap();
        for table in REQUIRED_TABLES {
            assert!(schema.tables.contains_key(*table), "missing table {table}");
            assert!(
                schema
                    .columns
                    .keys()
                    .any(|(candidate, _)| candidate == table),
                "missing parsed columns for {table}"
            );
            assert!(
                schema
                    .indexes
                    .contains_key(&(table.to_string(), "PRIMARY".to_string())),
                "missing parsed primary key for {table}"
            );
        }
    }

    #[test]
    fn parser_captures_defaults_extra_table_options_and_foreign_key_actions() {
        let schema = expected_schema().unwrap();
        assert_eq!(
            schema.tables.get("sys_user"),
            Some(&ExpectedTable {
                engine: "innodb".into(),
                character_set: "utf8mb4".into(),
                collation: "utf8mb4_general_ci".into(),
            })
        );
        assert_eq!(
            schema
                .columns
                .get(&("sys_user".into(), "tenant_id".into()))
                .unwrap()
                .default
                .as_deref(),
            Some("system")
        );
        assert_eq!(
            schema
                .columns
                .get(&("sys_user".into(), "updated_at".into()))
                .unwrap()
                .extra,
            "on update current_timestamp"
        );
        let username = schema
            .columns
            .get(&("sys_user".into(), "username".into()))
            .unwrap();
        assert_eq!(username.character_set.as_deref(), Some("utf8mb4"));
        assert_eq!(username.collation.as_deref(), Some("utf8mb4_general_ci"));
        assert!(username.generation_expression.is_empty());
        let foreign_key = schema
            .foreign_keys
            .get(&(
                "sys_role_permission".into(),
                "fk_sys_role_permission_permission".into(),
            ))
            .unwrap();
        assert_eq!(foreign_key.columns, ["perm_id"]);
        assert_eq!(foreign_key.referenced_table, "sys_permission");
        assert_eq!(foreign_key.referenced_columns, ["id"]);
        assert_eq!(foreign_key.update_rule, "cascade");
        assert_eq!(foreign_key.delete_rule, "cascade");
    }

    #[test]
    fn parser_handles_type_without_whitespace_before_nullability() {
        let schema = expected_schema().unwrap();
        assert_eq!(
            schema
                .columns
                .get(&("sys_file".into(), "file_url".into()))
                .unwrap()
                .column_type,
            "varchar(1000)"
        );
    }

    #[test]
    fn parser_does_not_treat_comment_parentheses_as_part_of_the_column_type() {
        let schema = expected_schema().unwrap();
        let parent_id = schema
            .columns
            .get(&("sys_menu".into(), "parent_id".into()))
            .unwrap();
        assert_eq!(parent_id.column_type, "bigint");

        let oper_param = schema
            .columns
            .get(&("sys_oper_log".into(), "oper_param".into()))
            .unwrap();
        assert_eq!(oper_param.column_type, "text");
        assert_eq!(oper_param.character_set.as_deref(), Some("utf8mb4"));
        assert_eq!(oper_param.collation.as_deref(), Some("utf8mb4_general_ci"));
    }

    #[test]
    fn parser_skips_blank_lines_before_foreign_key_actions() {
        let schema = expected_schema().unwrap();
        let foreign_key = schema
            .foreign_keys
            .get(&("sys_login_info".into(), "fk_sys_login_info_tenant".into()))
            .unwrap();
        assert_eq!(foreign_key.update_rule, "cascade");
        assert_eq!(foreign_key.delete_rule, "restrict");
    }

    #[test]
    fn legacy_allowlist_is_narrow_and_explicit() {
        assert!(is_upgrade_column("sys_menu", "route_key"));
        assert!(!is_upgrade_column("sys_menu", "name"));
        assert!(!is_legacy_extra_foreign_key(
            "sys_user_role",
            "fk_sys_user_role_user"
        ));
        assert!(!is_legacy_extra_foreign_key(
            "sys_role_permission",
            "unexpected"
        ));
    }

    #[test]
    fn actual_extra_normalization_ignores_only_mysql_metadata_noise() {
        assert_eq!(
            normalize_actual_extra("DEFAULT_GENERATED on update CURRENT_TIMESTAMP()"),
            "on update current_timestamp"
        );
        assert_eq!(
            normalize_actual_extra("STORED GENERATED"),
            "stored generated"
        );
    }
}
