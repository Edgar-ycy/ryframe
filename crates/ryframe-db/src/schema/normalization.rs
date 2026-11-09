// Rust 读取与 SQL 断言共同维护列规范化规则；不改变 canonical 表示。
const TIMESTAMP_DEFAULT: &str = "current_timestamp(6)";
const DEFAULT_GENERATED: &str = "default_generated";

pub fn normalize_column_default(value: Option<&str>) -> Option<&str> {
    value.map(|value| {
        if value.eq_ignore_ascii_case(TIMESTAMP_DEFAULT) {
            TIMESTAMP_DEFAULT
        } else {
            value
        }
    })
}

pub fn normalize_column_extra(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .split_whitespace()
        .filter(|part| *part != DEFAULT_GENERATED)
        .collect::<Vec<_>>()
        .join(" ")
}

/// MySQL LOWER 会转换非 ASCII 字符；canonical 使用 to_ascii_lowercase，必须逐字节等价。
pub(crate) fn ascii_lower(expression: &str) -> String {
    (b'A'..=b'Z').fold(expression.to_owned(), |sql, upper| {
        format!(
            "REPLACE({sql}, '{}', '{}')",
            char::from(upper),
            char::from(upper + 32)
        )
    })
}

pub(super) fn default_expression() -> String {
    format!(
        "IF(CAST({} AS BINARY) = CAST('{TIMESTAMP_DEFAULT}' AS BINARY), '{TIMESTAMP_DEFAULT}', column_default)",
        ascii_lower("column_default")
    )
}

pub(crate) fn extra_expression() -> String {
    // EXTRA 为 MySQL 生成的 ASCII 属性 token，不处理用户提供的 SQL 表达式。
    format!(
        "TRIM(REGEXP_REPLACE(REGEXP_REPLACE({}, '(^|[[:space:]]){DEFAULT_GENERATED}([[:space:]]|$)', ' '), '[[:space:]]+', ' '))",
        ascii_lower("extra")
    )
}
