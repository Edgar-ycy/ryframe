use sea_orm::DbErr;

use super::{canonical::Canonical, metadata};

/// 分类别的精确判定；由导出编排在带定位信息的失败 guard 中使用。
pub struct SchemaAssertion {
    pub category: &'static str,
    pub sql: String,
}

/// 完整 v2 结构断言；每个类别均比较实际 metadata，不解释迁移 DDL。
pub struct SchemaAssertions {
    pub predicates: Vec<SchemaAssertion>,
}

pub fn schema_assertions(table: &str, expected: &str) -> Result<SchemaAssertions, DbErr> {
    identifier(table)?;
    let schema = Canonical::decode(table, expected)?;
    Ok(SchemaAssertions {
        predicates: table_predicates(&schema),
    })
}

/// 初始表要求不存在；增量迁移使用声明的 before，不推测或读取 DDL 文本。
pub fn schema_preconditions(table: &str, before: Option<&str>) -> Result<SchemaAssertions, DbErr> {
    match before {
        Some(expected) => schema_assertions(table, expected),
        None => Ok(SchemaAssertions {
            predicates: vec![SchemaAssertion {
                category: "initial_absent",
                sql: initial_table_absent(table)?,
            }],
        }),
    }
}

impl SchemaAssertion {
    /// SQL 本身不含写账本或提交；调用方必须在完整 CHECK 核验后才能登记。
    pub fn diagnostic_guard(&self, migration: &str, phase: &str) -> Vec<String> {
        vec![
            format!(
                "SELECT {} AS migration, {} AS phase, {} AS schema_category",
                literal(migration),
                literal(phase),
                literal(self.category)
            ),
            format!(
                "INSERT INTO ryframe_schema_assertion_guard(valid) SELECT IF(({}), 1, 0)",
                self.sql
            ),
            "DELETE FROM ryframe_schema_assertion_guard".into(),
        ]
    }
}

/// 在任何登记事务开始前创建；临时对象仅属于当前导入连接。
pub fn schema_assertion_guard() -> &'static str {
    "CREATE TEMPORARY TABLE ryframe_schema_assertion_guard (valid TINYINT NOT NULL, CONSTRAINT ryframe_schema_assertion_failed CHECK (valid = 1)) ENGINE=InnoDB"
}

fn table_predicates(schema: &Canonical) -> Vec<SchemaAssertion> {
    let table = &schema.table.table_name;
    let mut result = vec![rows(
        "table",
        metadata::TABLE,
        table,
        vec![vec![
            eq("table_name", Some(table)),
            lower("engine", &schema.table.engine),
            lower("character_set_name", &schema.table.character_set_name),
            lower("table_collation", &schema.table.table_collation),
        ]],
    )];
    result.push(column_predicate(schema));
    result.push(rows(
        "indexes",
        metadata::INDEXES,
        table,
        schema
            .indexes
            .iter()
            .map(|i| {
                vec![
                    eq("index_name", Some(&i.index_name)),
                    eq("column_name", Some(&i.column_name)),
                    format!("seq_in_index = {}", i.seq_in_index),
                    format!("non_unique = {}", i.non_unique),
                    lower("index_type", &i.index_type),
                    format!(
                        "sub_part <=> {}",
                        i.sub_part.map_or_else(|| "NULL".into(), |n| n.to_string())
                    ),
                    eq("is_visible", Some(&i.is_visible)),
                ]
            })
            .collect(),
    ));
    result.push(rows(
        "constraints",
        metadata::CONSTRAINTS,
        table,
        schema
            .constraints
            .iter()
            .map(|c| {
                vec![
                    eq("constraint_name", Some(&c.constraint_name)),
                    eq("constraint_type", Some(&c.constraint_type)),
                    eq("enforced", Some(&c.enforced)),
                ]
            })
            .collect(),
    ));
    result.push(foreign_key_predicate(schema));
    let normalized = crate::migration::normalize_check_clause_sql("check_clause");
    result.push(rows(
        "checks",
        metadata::CHECKS,
        table,
        schema
            .checks
            .iter()
            .map(|c| {
                vec![
                    eq("constraint_name", Some(&c.constraint_name)),
                    eq(&normalized, Some(&c.check_clause)),
                ]
            })
            .collect(),
    ));
    result
}

fn foreign_key_predicate(schema: &Canonical) -> SchemaAssertion {
    let table = &schema.table.table_name;
    rows(
        "foreign_keys",
        metadata::FOREIGN_KEYS,
        table,
        schema
            .foreign_keys
            .iter()
            .map(|f| {
                vec![
                    eq("constraint_name", Some(&f.constraint_name)),
                    eq("column_name", Some(&f.column_name)),
                    format!("ordinal_position = {}", f.ordinal_position),
                    "CAST(referenced_table_schema AS BINARY) = CAST(current_schema AS BINARY)"
                        .into(),
                    eq("referenced_table_name", Some(&f.referenced_table_name)),
                    eq("referenced_column_name", Some(&f.referenced_column_name)),
                    eq("update_rule", Some(&f.update_rule)),
                    eq("delete_rule", Some(&f.delete_rule)),
                ]
            })
            .collect(),
    )
}

fn column_predicate(schema: &Canonical) -> SchemaAssertion {
    let table = &schema.table.table_name;
    rows(
        "columns",
        metadata::COLUMNS,
        table,
        schema
            .columns
            .iter()
            .enumerate()
            .map(|(position, c)| {
                vec![
                    format!("schema_order = {}", position + 1),
                    eq("column_name", Some(&c.column_name)),
                    lower("column_type", &c.column_type),
                    eq("is_nullable", Some(&c.is_nullable)),
                    eq_lower("character_set_name", c.character_set_name.as_deref()),
                    eq_lower("collation_name", c.collation_name.as_deref()),
                    eq("column_key", Some(&c.column_key)),
                    normalized_default(c.column_default.as_deref()),
                    normalized_extra(&c.extra),
                    eq("generation_expression", Some(&c.generation_expression)),
                ]
            })
            .collect(),
    )
}

fn rows(
    category: &'static str,
    query: &str,
    table: &str,
    expected: Vec<Vec<String>>,
) -> SchemaAssertion {
    rows_with_order(category, query, table, expected, true)
}

pub(crate) fn framework_rows(
    category: &'static str,
    table: &str,
    expected: Vec<Vec<String>>,
) -> SchemaAssertion {
    let query = match category {
        "table" => metadata::TABLE,
        "columns" => metadata::COLUMNS,
        "indexes" => metadata::INDEXES,
        "constraints" => metadata::CONSTRAINTS,
        "checks" => metadata::CHECKS,
        "foreign_keys" => metadata::FOREIGN_KEYS,
        _ => unreachable!("框架断言类别为内部固定集合"),
    };
    rows_with_order(category, query, table, expected, false)
}

fn rows_with_order(
    category: &'static str,
    query: &str,
    table: &str,
    expected: Vec<Vec<String>>,
    ordered: bool,
) -> SchemaAssertion {
    let query = query.replace('?', &literal(table));
    let mut tests = vec![format!(
        "(SELECT COUNT(*) FROM ({query}) AS actual) = {}",
        expected.len()
    )];
    for (position, mut fields) in expected.into_iter().enumerate() {
        if ordered
            && matches!(
                category,
                "indexes" | "constraints" | "foreign_keys" | "checks"
            )
        {
            fields.push(format!("schema_order = {}", position + 1));
        }
        tests.push(format!(
            "(SELECT COUNT(*) FROM ({query}) AS actual WHERE {}) = 1",
            fields.join(" AND ")
        ));
    }
    SchemaAssertion {
        category,
        sql: tests.join(" AND "),
    }
}

pub(crate) fn eq(field: &str, value: Option<&str>) -> String {
    format!(
        "CAST({field} AS BINARY) <=> {}",
        value.map_or_else(
            || "NULL".into(),
            |s| format!("CAST({} AS BINARY)", literal(s))
        )
    )
}

fn eq_lower(field: &str, value: Option<&str>) -> String {
    eq(&super::normalization::ascii_lower(field), value)
}

fn lower(field: &str, value: &str) -> String {
    eq_lower(field, Some(value))
}

fn normalized_default(value: Option<&str>) -> String {
    eq(&super::normalization::default_expression(), value)
}

fn normalized_extra(value: &str) -> String {
    eq(&super::normalization::extra_expression(), Some(value))
}

pub(crate) fn literal(value: &str) -> String {
    let hex = value
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!("CONVERT(X'{hex}' USING utf8mb4)")
}

pub fn initial_table_absent(table: &str) -> Result<String, DbErr> {
    identifier(table)?;
    Ok(format!(
        "(SELECT COUNT(*) FROM information_schema.tables WHERE table_schema=DATABASE() AND CAST(table_name AS BINARY)=CAST({} AS BINARY)) = 0",
        literal(table)
    ))
}

/// 登记阶段的事务开始与表锁分开保存，允许已有事务调用方复用同一锁语句。
/// 调用方负责在全部结构断言和账本写入成功后 COMMIT，失败时 ROLLBACK。
pub struct SchemaRegistrationLocks {
    pub begin: [&'static str; 2],
    pub tables: Vec<String>,
}

/// 仅生成最后一条 DDL 之后的登记事务起始与 metadata 锁，不执行或承诺 DDL 回滚。
pub fn schema_registration_locks<'a>(
    tables: impl IntoIterator<Item = &'a str>,
) -> Result<SchemaRegistrationLocks, DbErr> {
    let tables = tables
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let mut locks = Vec::new();
    for table in tables {
        identifier(table)?;
        locks.push(format!("SELECT 1 FROM `{table}` LIMIT 0"));
    }
    Ok(SchemaRegistrationLocks {
        begin: [
            "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
            "START TRANSACTION READ WRITE",
        ],
        tables: locks,
    })
}

fn identifier(table: &str) -> Result<(), DbErr> {
    if table.is_empty()
        || table.len() > 64
        || !table
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(DbErr::Custom("结构断言表名无效；尚未生成初始化 SQL".into()));
    }
    Ok(())
}
