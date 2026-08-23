pub fn extract_column_type(value: &str) -> &str {
    let value = value.trim_start();
    let first_whitespace = value.find(char::is_whitespace).unwrap_or(value.len());
    let base_end = if let Some(open) = value[..first_whitespace].find('(')
        && let Some(close) = value[open + 1..].find(')')
    {
        open + close + 2
    } else {
        first_whitespace
    };
    let remainder = &value[base_end..];
    let modifier = remainder.trim_start();
    let modifier_end = modifier.find(char::is_whitespace).unwrap_or(modifier.len());
    if modifier[..modifier_end].eq_ignore_ascii_case("UNSIGNED") {
        return &value[..base_end + (remainder.len() - modifier.len()) + modifier_end];
    }
    &value[..base_end]
}

pub fn normalize_column_type(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect()
}

pub fn expected_extra(value: &str) -> String {
    let lower = value
        .to_ascii_lowercase()
        .replace("current_timestamp()", "current_timestamp");
    let mut parts = Vec::new();
    if lower.contains("auto_increment") {
        parts.push("auto_increment".to_owned());
    }
    if let Some((_, update)) = lower.split_once("on update ")
        && let Some(function) = update.split_whitespace().next()
        && function.starts_with("current_timestamp")
    {
        parts.push(format!("on update {function}"));
    }
    parts.join(" ")
}

pub(super) fn normalize_identifier(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

pub(super) fn normalize_default(value: &str) -> String {
    let value = value.trim();
    if value.eq_ignore_ascii_case("current_timestamp")
        || value.eq_ignore_ascii_case("current_timestamp()")
    {
        "current_timestamp".into()
    } else {
        value.to_owned()
    }
}

pub(super) fn normalize_actual_extra(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .replace("current_timestamp()", "current_timestamp")
        .split_whitespace()
        .filter(|part| *part != "default_generated")
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn normalize_generation_expression(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|character| !character.is_ascii_whitespace() && *character != '`')
        .collect()
}

pub(super) fn normalize_action(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    if value == "no action" {
        "restrict".into()
    } else {
        value
    }
}

pub(super) fn compatible_column_type(expected: &str, actual: &str) -> bool {
    expected == actual
        || matches!(
            (expected, actual),
            ("tinyint(1)", "tinyint") | ("tinyint", "tinyint(1)")
        )
}

pub(super) fn nullable_label(nullable: bool) -> &'static str {
    if nullable { "NULL" } else { "NOT NULL" }
}
