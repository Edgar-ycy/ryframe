use sea_orm::DbErr;

mod clauses;

use super::{
    normalize::{
        expected_extra, extract_column_type, normalize_action, normalize_column_type,
        normalize_default, normalize_generation_expression, normalize_identifier,
    },
    types::{ExpectedColumn, ExpectedForeignKey, ExpectedIndex, ExpectedSchema, ExpectedTable},
};
use crate::migration::baseline_contract::ddl_statements;

pub(super) fn expected_schema() -> Result<ExpectedSchema, DbErr> {
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
        add_table_parts(
            &mut schema,
            &table,
            statement,
            &table_character_set,
            &table_collation,
        )?;
    }
    add_post_baseline_constraints(&mut schema);
    Ok(schema)
}

fn add_table_parts(
    schema: &mut ExpectedSchema,
    table: &str,
    statement: &str,
    table_character_set: &str,
    table_collation: &str,
) -> Result<(), DbErr> {
    for line in clauses::table_clauses(statement)? {
        let upper = line.to_ascii_uppercase();
        if line.starts_with('`') {
            add_column(
                schema,
                table,
                line,
                &upper,
                table_character_set,
                table_collation,
            );
        } else if upper.starts_with("PRIMARY KEY") {
            schema.indexes.insert(
                (table.into(), "PRIMARY".into()),
                ExpectedIndex {
                    unique: true,
                    columns: backtick_identifiers(line),
                },
            );
        } else if upper.starts_with("UNIQUE KEY") || upper.starts_with("KEY ") {
            add_named_index(schema, table, line, &upper);
        }
        if (upper.starts_with("CONSTRAINT ") || upper.starts_with("FOREIGN KEY"))
            && let Some(offset) = upper.find("FOREIGN KEY")
        {
            let name = upper
                .starts_with("CONSTRAINT ")
                .then(|| backtick_identifiers(&line[..offset]).into_iter().next())
                .flatten()
                .ok_or_else(|| {
                    DbErr::Custom(format!(
                        "foreign key in {table} is missing a constraint name"
                    ))
                })?;
            add_foreign_key(schema, table, name, &line[offset..])?;
        }
    }
    Ok(())
}

fn add_column(
    schema: &mut ExpectedSchema,
    table: &str,
    line: &str,
    upper: &str,
    table_character_set: &str,
    table_collation: &str,
) {
    let identifiers = backtick_identifiers(line);
    let Some(column) = identifiers.first() else {
        return;
    };
    let after_name = line
        .split_once('`')
        .and_then(|(_, rest)| rest.split_once('`'))
        .map(|(_, rest)| rest.trim_start())
        .unwrap_or_default();
    let column_type = normalize_column_type(extract_column_type(after_name));
    let uses_character_set = column_type_uses_character_set(&column_type);
    schema.columns.insert(
        (table.into(), column.clone()),
        ExpectedColumn {
            column_type,
            nullable: !upper.contains("NOT NULL"),
            default: extract_column_default(after_name),
            extra: expected_extra(after_name),
            character_set: uses_character_set.then(|| {
                extract_identifier_option(after_name, "CHARACTER SET")
                    .unwrap_or_else(|| table_character_set.into())
            }),
            collation: uses_character_set.then(|| {
                extract_identifier_option(after_name, "COLLATE")
                    .unwrap_or_else(|| table_collation.into())
            }),
            generation_expression: expected_generation_expression(after_name),
        },
    );
}

fn add_named_index(schema: &mut ExpectedSchema, table: &str, line: &str, upper: &str) {
    let identifiers = backtick_identifiers(line);
    if let Some((name, columns)) = identifiers.split_first() {
        schema.indexes.insert(
            (table.into(), name.clone()),
            ExpectedIndex {
                unique: upper.starts_with("UNIQUE KEY"),
                columns: columns.to_vec(),
            },
        );
    }
}

fn add_foreign_key(
    schema: &mut ExpectedSchema,
    table: &str,
    name: String,
    clause: &str,
) -> Result<(), DbErr> {
    let clause_upper = clause.to_ascii_uppercase();
    let reference_at = clause_upper.find("REFERENCES").ok_or_else(|| {
        DbErr::Custom(format!("foreign key {table}.{name} is missing REFERENCES"))
    })?;
    let columns = backtick_identifiers(&clause[..reference_at]);
    let referenced = backtick_identifiers(&clause[reference_at..]);
    let (referenced_table, referenced_columns) = referenced.split_first().ok_or_else(|| {
        DbErr::Custom(format!("foreign key {table}.{name} has an invalid target"))
    })?;
    schema.foreign_keys.insert(
        (table.into(), name),
        ExpectedForeignKey {
            columns,
            referenced_table: referenced_table.clone(),
            referenced_columns: referenced_columns.to_vec(),
            update_rule: extract_action(clause, "ON UPDATE").unwrap_or_else(|| "restrict".into()),
            delete_rule: extract_action(clause, "ON DELETE").unwrap_or_else(|| "restrict".into()),
        },
    );
    Ok(())
}

/// 补充必须等被引用表创建完成后再由同一基线安装的跨表约束。
fn add_post_baseline_constraints(schema: &mut ExpectedSchema) {
    schema.foreign_keys.insert(
        ("sys_user".into(), "fk_user_avatar_file".into()),
        ExpectedForeignKey {
            columns: vec!["avatar_file_id".into()],
            referenced_table: "sys_file".into(),
            referenced_columns: vec!["id".into()],
            update_rule: "cascade".into(),
            delete_rule: "restrict".into(),
        },
    );
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

#[cfg(test)]
mod tests {
    use super::{ExpectedSchema, add_table_parts};

    fn parse_table(statement: &str) -> ExpectedSchema {
        let mut schema = ExpectedSchema::default();
        add_table_parts(
            &mut schema,
            "child",
            statement,
            "utf8mb4",
            "utf8mb4_general_ci",
        )
        .unwrap();
        schema
    }

    #[test]
    fn synthetic_multiline_check_and_foreign_key_keep_complete_clauses() {
        let schema = parse_table(
            r#"CREATE TABLE `child` (
                `id` BIGINT NOT NULL,
                `parent_id` BIGINT NOT NULL,
                `state` VARCHAR(16) NOT NULL DEFAULT 'queued,ready',
                CONSTRAINT `ck_child_state` CHECK (
                    (`state` IN ('queued,ready', 'done'))
                    AND (`id` > 0)),
                CONSTRAINT `fk_child_parent`
                    FOREIGN KEY (`parent_id`)
                    REFERENCES `parent` (`id`)
                    ON DELETE CASCADE ON UPDATE RESTRICT
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci"#,
        );

        assert_eq!(
            schema
                .columns
                .keys()
                .filter(|(table, _)| table == "child")
                .count(),
            3
        );
        let key = &schema.foreign_keys[&("child".into(), "fk_child_parent".into())];
        assert_eq!(key.columns, ["parent_id"]);
        assert_eq!(key.referenced_table, "parent");
        assert_eq!(key.referenced_columns, ["id"]);
        assert_eq!(key.delete_rule, "cascade");
        assert_eq!(key.update_rule, "restrict");
    }

    #[test]
    fn synthetic_inline_foreign_key_keeps_actions() {
        let schema = parse_table(
            r#"CREATE TABLE `child` (
                `id` BIGINT NOT NULL,
                `owner_id` BIGINT NULL,
                CONSTRAINT `fk_child_owner` FOREIGN KEY (`owner_id`) REFERENCES `owner` (`id`) ON DELETE SET NULL ON UPDATE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci"#,
        );

        let key = &schema.foreign_keys[&("child".into(), "fk_child_owner".into())];
        assert_eq!(key.columns, ["owner_id"]);
        assert_eq!(key.referenced_table, "owner");
        assert_eq!(key.referenced_columns, ["id"]);
        assert_eq!(key.delete_rule, "set null");
        assert_eq!(key.update_rule, "cascade");
    }

    #[test]
    fn multiline_checks_preserve_attempt_columns_and_foreign_key() {
        let schema = super::expected_schema().unwrap();
        let table = "sys_background_job_attempt";
        let columns = schema
            .columns
            .iter()
            .filter(|((name, _), _)| name == table)
            .collect::<Vec<_>>();
        assert_eq!(columns.len(), 7);
        for name in ["available_at", "started_at"] {
            let column = &schema.columns[&(table.into(), name.into())];
            assert_eq!(column.column_type, "datetime(6)");
            assert!(!column.nullable);
        }
        for name in ["finished_at", "closed_at"] {
            let column = &schema.columns[&(table.into(), name.into())];
            assert_eq!(column.column_type, "datetime(6)");
            assert!(column.nullable);
        }
        let key = &schema.foreign_keys[&(table.into(), "fk_bg_attempt_job".into())];
        assert_eq!(key.columns, ["job_id"]);
        assert_eq!(key.referenced_table, "sys_background_job");
        assert_eq!(key.delete_rule, "cascade");
        assert_eq!(key.update_rule, "restrict");
    }

    #[test]
    fn inline_backup_constraints_are_verified_against_their_columns_and_target() {
        let schema = super::expected_schema().unwrap();
        for (table, name) in [
            ("sys_backup_resource", "fk_backup_resource_set"),
            ("sys_restore_run", "fk_restore_run_backup"),
        ] {
            let key = schema
                .foreign_keys
                .get(&(table.into(), name.into()))
                .unwrap();
            assert_eq!(key.columns, ["backup_id"]);
            assert_eq!(key.referenced_table, "sys_backup_set");
            assert_eq!(key.referenced_columns, ["id"]);
            assert_eq!(key.delete_rule, "restrict");
            assert_eq!(key.update_rule, "restrict");
        }
    }
}
