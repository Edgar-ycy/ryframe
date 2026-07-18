use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement, TryGetable};

use crate::m20260522_000000_mysql_baseline::{ddl_statements, seed_statements};

/// Insert canonical bootstrap records without overwriting operational changes.
///
/// Duplicate keys use an explicit no-op upsert; unrelated SQL errors are never
/// suppressed. Every canonical semantic identity and relationship is verified
/// after the write, so a conflicting primary or unique key cannot masquerade as
/// a successful bootstrap.
pub async fn seed<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if db.get_database_backend() != DbBackend::MySql {
        return Err(DbErr::Custom("RyFrame v0.5 only supports MySQL".into()));
    }
    for statement in seed_statements() {
        db.execute_unprepared(&idempotent_upsert(statement)?)
            .await?;
    }
    verify_seed_identities(db).await?;
    verify_seed_relationships(db).await
}

pub fn mysql_snapshot_sql() -> String {
    let mut snapshot = String::from(
        "-- GENERATED FILE: RyFrame v0.5 canonical MySQL schema snapshot.\n\
         -- Source of truth: ryframe-db-migration Migrator + Seeder.\n\
         -- REVIEW ONLY: deployment and reset tools must never execute this file.\n\
         -- Regenerate with: cargo run -p ryframe-db-migration --bin export_mysql_snapshot -- sql/ryframe_config.sql\n\n",
    );
    for statement in ddl_statements() {
        snapshot.push_str(statement.trim());
        snapshot.push_str(";\n\n");
    }
    snapshot.push_str("-- Idempotent bootstrap data (production users start locked).\n\n");
    for statement in seed_statements() {
        snapshot.push_str(
            &idempotent_upsert(statement)
                .expect("canonical seed statements must be valid INSERT statements"),
        );
        snapshot.push_str(";\n\n");
    }
    snapshot.truncate(snapshot.trim_end().len());
    snapshot.push('\n');
    snapshot
}

#[derive(Debug)]
struct SeedInsert {
    table: String,
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
}

fn idempotent_upsert(statement: &str) -> Result<String, DbErr> {
    let parsed = parse_seed_insert(statement)?;
    let no_op_column = parsed.columns.first().ok_or_else(|| {
        DbErr::Custom(format!(
            "canonical seed for {} has no columns",
            parsed.table
        ))
    })?;
    Ok(format!(
        "{} ON DUPLICATE KEY UPDATE `{no_op_column}` = `{no_op_column}`",
        statement.trim()
    ))
}

async fn verify_seed_identities<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    for statement in seed_statements() {
        let insert = parse_seed_insert(statement)?;
        let identity_columns = seed_identity_columns(&insert.table).ok_or_else(|| {
            DbErr::Custom(format!(
                "canonical seed table {} has no identity verification specification",
                insert.table
            ))
        })?;

        for row in &insert.rows {
            let mut predicates = Vec::with_capacity(identity_columns.len() + 1);
            for identity_column in identity_columns {
                let position = insert
                    .columns
                    .iter()
                    .position(|column| column == identity_column)
                    .ok_or_else(|| {
                        DbErr::Custom(format!(
                            "canonical seed for {} omits identity column {}",
                            insert.table, identity_column
                        ))
                    })?;
                predicates.push(format!(
                    "`{identity_column}` <=> {}",
                    row.get(position).ok_or_else(|| {
                        DbErr::Custom(format!(
                            "canonical seed for {} has a malformed value tuple",
                            insert.table
                        ))
                    })?
                ));
            }
            if insert.table != "sys_tenant" && !identity_columns.contains(&"tenant_id") {
                predicates.push("`tenant_id` <=> 'system'".into());
            }
            let sql = format!(
                "SELECT COUNT(*) FROM `{}` WHERE {}",
                insert.table,
                predicates.join(" AND ")
            );
            let row = db
                .query_one_raw(Statement::from_string(DbBackend::MySql, sql))
                .await?
                .ok_or_else(|| {
                    DbErr::Custom(format!(
                        "seed identity verification returned no row for {}",
                        insert.table
                    ))
                })?;
            let count = i64::try_get_by_index(&row, 0)?;
            if count != 1 {
                return Err(DbErr::Custom(format!(
                    "canonical seed identity is missing or conflicting in {}: expected one row matching {}; refusing startup",
                    insert.table,
                    predicates.join(" AND ")
                )));
            }
        }
    }
    Ok(())
}

async fn verify_seed_relationships<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    const RELATIONSHIPS: &[(&str, &str)] = &[
        (
            "SELECT COUNT(*) FROM sys_user_role ur \
             LEFT JOIN sys_user u ON u.id = ur.user_id AND u.tenant_id = ur.tenant_id \
             LEFT JOIN sys_role r ON r.id = ur.role_id AND r.tenant_id = ur.tenant_id \
             WHERE ur.tenant_id = 'system' AND (u.id IS NULL OR r.id IS NULL)",
            "system user-role bindings",
        ),
        (
            "SELECT COUNT(*) FROM sys_role_permission rp \
             LEFT JOIN sys_role r ON r.id = rp.role_id AND r.tenant_id = rp.tenant_id \
             LEFT JOIN sys_permission p ON p.id = rp.perm_id AND p.tenant_id = rp.tenant_id \
             WHERE rp.tenant_id = 'system' AND (r.id IS NULL OR p.id IS NULL)",
            "system role-permission bindings",
        ),
        (
            "SELECT COUNT(*) FROM sys_menu m \
             LEFT JOIN sys_menu parent ON parent.id = m.parent_id AND parent.tenant_id = m.tenant_id \
             LEFT JOIN sys_permission p ON p.id = m.perm_id AND p.tenant_id = m.tenant_id \
             WHERE m.tenant_id = 'system' \
               AND ((m.parent_id IS NOT NULL AND parent.id IS NULL) \
                 OR (m.perm_id IS NOT NULL AND p.id IS NULL))",
            "system menu hierarchy and permissions",
        ),
        (
            "SELECT COUNT(*) FROM sys_permission p \
             LEFT JOIN sys_permission parent \
               ON parent.id = p.parent_id AND parent.tenant_id = p.tenant_id \
             WHERE p.tenant_id = 'system' \
               AND p.parent_id IS NOT NULL AND parent.id IS NULL",
            "system permission hierarchy",
        ),
        (
            "SELECT COUNT(*) FROM sys_dept d \
             LEFT JOIN sys_dept parent ON parent.id = d.parent_id AND parent.tenant_id = d.tenant_id \
             WHERE d.tenant_id = 'system' \
               AND d.parent_id IS NOT NULL AND parent.id IS NULL",
            "system department hierarchy",
        ),
    ];

    for (sql, label) in RELATIONSHIPS {
        let row = db
            .query_one_raw(Statement::from_string(DbBackend::MySql, (*sql).to_owned()))
            .await?
            .ok_or_else(|| {
                DbErr::Custom(format!(
                    "seed relationship verification returned no row for {label}"
                ))
            })?;
        let violations = i64::try_get_by_index(&row, 0)?;
        if violations != 0 {
            return Err(DbErr::Custom(format!(
                "seed relationship verification failed for {label}: found {violations} invalid rows; refusing startup"
            )));
        }
    }
    Ok(())
}

fn seed_identity_columns(table: &str) -> Option<&'static [&'static str]> {
    match table {
        "sys_tenant" => Some(&["id", "tenant_id"]),
        "sys_dept" => Some(&["id", "parent_id"]),
        "sys_role" => Some(&["id", "code"]),
        "sys_user" => Some(&["id", "username"]),
        "sys_permission" => Some(&["id", "code", "parent_id"]),
        "sys_menu" => Some(&["id", "parent_id", "menu_type", "perm_id", "route_key"]),
        "sys_post" => Some(&["id", "code"]),
        "sys_config" => Some(&["id", "key"]),
        "sys_dict_type" => Some(&["id", "code"]),
        "sys_dict_data" => Some(&["id", "type_code", "value"]),
        "sys_user_role" => Some(&["user_id", "role_id"]),
        "sys_role_permission" => Some(&["role_id", "perm_id"]),
        _ => None,
    }
}

fn parse_seed_insert(statement: &str) -> Result<SeedInsert, DbErr> {
    let statement = statement.trim();
    if !statement.to_ascii_uppercase().starts_with("INSERT INTO") {
        return Err(DbErr::Custom(
            "canonical seed contains a non-INSERT statement".into(),
        ));
    }
    let identifiers = backtick_identifiers(statement);
    let table = identifiers
        .first()
        .cloned()
        .ok_or_else(|| DbErr::Custom("canonical seed INSERT is missing a table name".into()))?;

    let open = statement.find('(').ok_or_else(|| {
        DbErr::Custom(format!(
            "canonical seed for {table} is missing a column list"
        ))
    })?;
    let close = statement[open + 1..]
        .find(')')
        .map(|index| open + index + 1)
        .ok_or_else(|| {
            DbErr::Custom(format!(
                "canonical seed for {table} has an unterminated column list"
            ))
        })?;
    let columns = backtick_identifiers(&statement[open..=close]);
    let after_columns = &statement[close + 1..];
    let values_at = after_columns
        .to_ascii_uppercase()
        .find("VALUES")
        .ok_or_else(|| DbErr::Custom(format!("canonical seed for {table} is missing VALUES")))?;
    let rows = split_value_rows(&after_columns[values_at + "VALUES".len()..])?;
    if columns.is_empty() || rows.is_empty() {
        return Err(DbErr::Custom(format!(
            "canonical seed for {table} has no columns or rows"
        )));
    }
    if rows.iter().any(|row| row.len() != columns.len()) {
        return Err(DbErr::Custom(format!(
            "canonical seed for {table} has a value count that does not match its columns"
        )));
    }
    Ok(SeedInsert {
        table,
        columns,
        rows,
    })
}

fn split_value_rows(value: &str) -> Result<Vec<Vec<String>>, DbErr> {
    let mut rows = Vec::new();
    let mut row_start = None;
    let mut depth = 0_usize;
    let mut quoted = false;
    let mut characters = value.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if character == '\'' {
            if quoted && characters.peek().is_some_and(|(_, next)| *next == '\'') {
                characters.next();
                continue;
            }
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }
        match character {
            '(' => {
                if depth == 0 {
                    row_start = Some(index + 1);
                }
                depth += 1;
            }
            ')' => {
                if depth == 0 {
                    return Err(DbErr::Custom(
                        "canonical seed has an unmatched closing parenthesis".into(),
                    ));
                }
                depth -= 1;
                if depth == 0 {
                    let start = row_start
                        .take()
                        .ok_or_else(|| DbErr::Custom("canonical seed row has no start".into()))?;
                    rows.push(split_row_values(&value[start..index])?);
                }
            }
            _ => {}
        }
    }
    if quoted || depth != 0 {
        return Err(DbErr::Custom(
            "canonical seed has an unterminated string or value tuple".into(),
        ));
    }
    Ok(rows)
}

fn split_row_values(row: &str) -> Result<Vec<String>, DbErr> {
    let mut values = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut characters = row.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if character == '\'' {
            if quoted && characters.peek().is_some_and(|(_, next)| *next == '\'') {
                characters.next();
                continue;
            }
            quoted = !quoted;
        } else if character == ',' && !quoted {
            values.push(row[start..index].trim().to_owned());
            start = index + 1;
        }
    }
    if quoted {
        return Err(DbErr::Custom(
            "canonical seed row has an unterminated string".into(),
        ));
    }
    values.push(row[start..].trim().to_owned());
    Ok(values)
}

fn backtick_identifiers(value: &str) -> Vec<String> {
    value
        .split('`')
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, identifier)| identifier.to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_is_generated_safe_and_mysql_only() {
        let snapshot = mysql_snapshot_sql();
        assert!(snapshot.starts_with("-- GENERATED FILE"));
        assert!(snapshot.contains("CREATE TABLE IF NOT EXISTS `sys_user`"));
        assert!(snapshot.contains("ON DUPLICATE KEY UPDATE `id` = `id`"));
        assert!(!snapshot.contains("INSERT IGNORE"));
        assert!(!snapshot.contains("DROP TABLE"));
        assert!(!snapshot.contains("FOREIGN_KEY_CHECKS"));
        assert!(!snapshot.to_ascii_lowercase().contains("postgres"));
        assert!(!snapshot.to_ascii_lowercase().contains("sqlite"));
        assert!(snapshot.ends_with('\n'));
        assert!(!snapshot.ends_with("\n\n"));
    }

    #[test]
    fn seed_upsert_only_suppresses_duplicate_keys() {
        assert_eq!(
            idempotent_upsert("INSERT INTO `example` (`id`) VALUES (1)").unwrap(),
            "INSERT INTO `example` (`id`) VALUES (1) ON DUPLICATE KEY UPDATE `id` = `id`"
        );
    }

    #[test]
    fn every_canonical_seed_row_has_a_complete_identity_specification() {
        let mut rows = 0;
        for statement in seed_statements() {
            let insert = parse_seed_insert(statement).unwrap();
            let identities = seed_identity_columns(&insert.table)
                .unwrap_or_else(|| panic!("missing identity spec for {}", insert.table));
            for identity in identities {
                assert!(
                    insert.columns.iter().any(|column| column == identity),
                    "{} omits identity column {identity}",
                    insert.table
                );
            }
            rows += insert.rows.len();
        }
        assert!(
            rows >= 200,
            "canonical seed unexpectedly shrank to {rows} rows"
        );
    }

    #[test]
    fn seed_parser_keeps_commas_and_escaped_quotes_inside_literals() {
        let parsed = parse_seed_insert(
            "INSERT INTO `example` (`id`, `label`) VALUES (1, 'alpha,beta'), (2, 'it''s')",
        )
        .unwrap();
        assert_eq!(parsed.rows[0], ["1", "'alpha,beta'"]);
        assert_eq!(parsed.rows[1], ["2", "'it''s'"]);
    }
}
