mod groups;
mod lex;

use crate::schema::{sql_ascii_lower as lower, sql_literal as literal};

/// 只读 SQL 与 Rust 使用相同 token 类型、最长符号、字面量及括号消除规则。
/// 返回 v2 CHECK token 编码，不执行 SQL；导入连接须显式使用服务器支持的最大递归深度，并在成功后恢复。
pub fn normalize_check_clause_sql(expression: &str) -> String {
    let lex = lex::cte();
    let groups = groups::cte();
    format!(
        r#"(WITH RECURSIVE
        src AS (SELECT CAST(({expression}) AS BINARY) AS raw),
        {lex}, {groups},
        encoded(n, value) AS (
            SELECT 0, JSON_ARRAY('')
            UNION ALL
            SELECT n+1, JSON_ARRAY(CONCAT(value->>'$[0]',
                JSON_UNQUOTE(JSON_EXTRACT(x, CONCAT('$[', n, '][0]'))),
                OCTET_LENGTH(UNHEX(JSON_UNQUOTE(JSON_EXTRACT(x, CONCAT('$[', n, '][1]'))))), ':',
                CONVERT(UNHEX(JSON_UNQUOTE(JSON_EXTRACT(x, CONCAT('$[', n, '][1]')))) USING utf8mb4), ';'))
            FROM encoded CROSS JOIN normalized WHERE n < JSON_LENGTH(x)
        ) SELECT value->>'$[0]' FROM encoded CROSS JOIN normalized WHERE n=JSON_LENGTH(x))"#
    )
}

fn get(field: &str) -> String {
    format!("state->>'$.{field}'")
}
fn number(field: &str) -> String {
    format!("CAST({} AS SIGNED)", get(field))
}
fn json(field: &str) -> String {
    format!("JSON_EXTRACT(state, '$.{field}')")
}

fn set(fields: &[(&str, String)]) -> String {
    format!(
        "JSON_SET(state, {})",
        fields
            .iter()
            .map(|(field, value)| format!("'$.{field}', {value}"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn choose(branches: &[(String, String)], otherwise: &str) -> String {
    format!(
        "CASE {} ELSE {otherwise} END",
        branches
            .iter()
            .map(|(condition, value)| format!("WHEN {condition} THEN {value}"))
            .collect::<Vec<_>>()
            .join(" ")
    )
}
