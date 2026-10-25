use super::{
    expected::{expected_schema, extend_schema},
    types::ExpectedSchema,
};
use crate::schema::{
    SchemaAssertion, SchemaAssertions, framework_rows, sql_ascii_lower as lower, sql_equal as eq,
    sql_literal as literal,
};
use sea_orm::DbErr;

/// 仅使用运行 verify 已有的框架期望模型；extra 是组合根同源租户基线建表定义。
/// 业务 CREATE/ALTER 不进入此解析路径，仍以注册的完整 canonical 为准。
pub fn framework_initialization_assertions(
    extra: &[&str],
) -> Result<Vec<(String, SchemaAssertions)>, DbErr> {
    let mut schema = expected_schema()?;
    extend_schema(&mut schema, extra.iter().copied())?;
    Ok(schema
        .tables
        .keys()
        .map(|table| {
            let predicates = vec![
                table_assertion(&schema, table),
                columns(&schema, table),
                indexes(&schema, table),
                foreign_keys(&schema, table),
                checks(&schema, table),
                constraints(&schema, table),
            ];
            (table.clone(), SchemaAssertions { predicates })
        })
        .collect())
}

fn table_assertion(s: &ExpectedSchema, table: &str) -> SchemaAssertion {
    let t = &s.tables[table];
    framework_rows(
        "table",
        table,
        vec![vec![
            eq("table_name", Some(table)),
            eq(&lower("engine"), Some(&t.engine)),
            eq(&lower("character_set_name"), Some(&t.character_set)),
            eq(&lower("table_collation"), Some(&t.collation)),
        ]],
    )
}

fn columns(s: &ExpectedSchema, table: &str) -> SchemaAssertion {
    let normalized_type = format!(
        "REGEXP_REPLACE({}, {}, '')",
        lower("column_type"),
        literal("[\t\n\u{c}\r ]+")
    );
    let default = format!(
        "IF(CAST({} AS BINARY) IN (CAST('current_timestamp' AS BINARY), CAST('current_timestamp()' AS BINARY)), 'current_timestamp', IF(CAST({} AS BINARY)=CAST('current_timestamp(6)' AS BINARY), 'current_timestamp(6)', column_default))",
        lower("column_default"),
        lower("column_default")
    );
    let extra = format!(
        "REPLACE({}, 'current_timestamp()', 'current_timestamp')",
        crate::schema::sql_column_extra()
    );
    let generated = format!(
        "REGEXP_REPLACE(REPLACE({}, '`', ''), {}, '')",
        lower("generation_expression"),
        literal("[\t\n\u{c}\r ]+")
    );
    framework_rows(
        "columns",
        table,
        s.columns
            .iter()
            .filter(|((t, _), _)| t == table)
            .map(|((_, name), c)| {
                let mut ty = eq(&normalized_type, Some(&c.column_type));
                if matches!(c.column_type.as_str(), "tinyint" | "tinyint(1)") {
                    ty = format!(
                        "({ty} OR {})",
                        eq(
                            &normalized_type,
                            Some(if c.column_type == "tinyint" {
                                "tinyint(1)"
                            } else {
                                "tinyint"
                            })
                        )
                    );
                }
                vec![
                    format!(
                        "schema_order={}",
                        s.column_order[&(table.to_owned(), name.clone())]
                    ),
                    eq("column_name", Some(name)),
                    ty,
                    eq("is_nullable", Some(if c.nullable { "YES" } else { "NO" })),
                    eq(
                        &default,
                        crate::schema::normalize_column_default(c.default.as_deref()),
                    ),
                    eq(&extra, Some(&c.extra)),
                    eq(&lower("character_set_name"), c.character_set.as_deref()),
                    eq(&lower("collation_name"), c.collation.as_deref()),
                    eq(&generated, Some(&c.generation_expression)),
                ]
            })
            .collect(),
    )
}

fn indexes(s: &ExpectedSchema, table: &str) -> SchemaAssertion {
    framework_rows(
        "indexes",
        table,
        s.indexes
            .iter()
            .filter(|((t, _), _)| t == table)
            .flat_map(|((_, name), i)| {
                i.columns.iter().enumerate().map(move |(position, column)| {
                    vec![
                        eq("index_name", Some(name)),
                        eq("column_name", Some(column)),
                        format!("seq_in_index={}", position + 1),
                        format!("non_unique={}", u8::from(!i.unique)),
                        eq(&lower("index_type"), Some("btree")),
                        "sub_part IS NULL".into(),
                        eq("is_visible", Some("YES")),
                    ]
                })
            })
            .collect(),
    )
}

fn foreign_keys(s: &ExpectedSchema, table: &str) -> SchemaAssertion {
    framework_rows("foreign_keys", table, s.foreign_keys.iter().filter(|((t,_),_)|t==table).flat_map(|((_, name), f)| f.columns.iter().zip(&f.referenced_columns).enumerate().map(move |(position, (column, referenced))| vec![eq("constraint_name", Some(name)), eq("column_name", Some(column)), format!("ordinal_position={}", position+1), "CAST(referenced_table_schema AS BINARY)=CAST(current_schema AS BINARY)".into(), eq("referenced_table_name", Some(&f.referenced_table)), eq("referenced_column_name", Some(referenced)), action("update_rule", &f.update_rule), action("delete_rule", &f.delete_rule)])).collect())
}

fn action(field: &str, expected: &str) -> String {
    eq(
        &format!(
            "IF(CAST({} AS BINARY)=CAST('no action' AS BINARY), 'restrict', {})",
            lower(field),
            lower(field)
        ),
        Some(expected),
    )
}

fn checks(s: &ExpectedSchema, table: &str) -> SchemaAssertion {
    let normalized = super::normalize::normalize_check_clause_sql("check_clause");
    framework_rows(
        "checks",
        table,
        s.checks
            .iter()
            .filter(|((t, _), _)| t == table)
            .map(|((_, name), c)| {
                vec![
                    eq("constraint_name", Some(name)),
                    eq(&normalized, Some(&c.clause)),
                ]
            })
            .collect(),
    )
}

fn constraints(s: &ExpectedSchema, table: &str) -> SchemaAssertion {
    let mut expected = Vec::new();
    for ((t, name), index) in &s.indexes {
        if t == table && index.unique {
            expected.push(vec![
                eq("constraint_name", Some(name)),
                eq(
                    "constraint_type",
                    Some(if name == "PRIMARY" {
                        "PRIMARY KEY"
                    } else {
                        "UNIQUE"
                    }),
                ),
                eq("enforced", Some("YES")),
            ]);
        }
    }
    for ((t, name), _) in &s.foreign_keys {
        if t == table {
            expected.push(vec![
                eq("constraint_name", Some(name)),
                eq("constraint_type", Some("FOREIGN KEY")),
                eq("enforced", Some("YES")),
            ]);
        }
    }
    for ((t, name), c) in &s.checks {
        if t == table {
            expected.push(vec![
                eq("constraint_name", Some(name)),
                eq("constraint_type", Some("CHECK")),
                eq("enforced", Some(if c.enforced { "YES" } else { "NO" })),
            ]);
        }
    }
    framework_rows("constraints", table, expected)
}
