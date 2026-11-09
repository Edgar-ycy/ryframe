use std::collections::BTreeSet;

use serde_json::Value;

use super::sql::{Row, SqlValue};
use crate::Result;

pub(super) mod frozen;

pub(crate) fn reject_row(
    row: &Row,
    forbidden: &[String],
    logical: &BTreeSet<String>,
) -> Result<()> {
    let forbidden = forbidden
        .iter()
        .map(|value| casefold(value))
        .collect::<Vec<_>>();
    for value in row.values() {
        inspect_sql(value, &forbidden, logical, true)?;
    }
    Ok(())
}

fn inspect_sql(
    value: &SqlValue,
    forbidden: &[String],
    logical: &BTreeSet<String>,
    allow_logical: bool,
) -> Result<()> {
    match value {
        SqlValue::Bytes(value) => {
            let value = std::str::from_utf8(value)
                .map_err(|_| "数据含未解决的来源物理 scope、库或端点引用")?;
            inspect_text(value, forbidden, logical, allow_logical)
        }
        SqlValue::Text(value) => inspect_text(value, forbidden, logical, allow_logical),
        SqlValue::Json(value) => inspect_json(value, forbidden, logical, allow_logical),
        SqlValue::Null | SqlValue::Number(_) => Ok(()),
    }
}

fn inspect_json(
    value: &Value,
    forbidden: &[String],
    logical: &BTreeSet<String>,
    allow_logical: bool,
) -> Result<()> {
    match value {
        Value::String(value) => inspect_text(value, forbidden, logical, allow_logical),
        Value::Array(values) => {
            for value in values {
                inspect_json(value, forbidden, logical, allow_logical)?;
            }
            Ok(())
        }
        Value::Object(values) => {
            for (key, value) in values {
                inspect_text(key, forbidden, logical, allow_logical)?;
                inspect_json(
                    value,
                    forbidden,
                    logical,
                    allow_logical && !is_physical_key(key),
                )?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn inspect_text(
    value: &str,
    forbidden: &[String],
    logical: &BTreeSet<String>,
    allow_logical: bool,
) -> Result<()> {
    let examined = if allow_logical {
        logical.iter().fold(value.to_owned(), |current, tenant| {
            replace_logical(&current, tenant)
        })
    } else {
        value.to_owned()
    };
    let folded = casefold(&examined);
    if forbidden.iter().any(|token| folded.contains(token)) {
        Err("数据含未解决的来源物理 scope、库或端点引用".into())
    } else {
        Ok(())
    }
}

fn is_physical_key(key: &str) -> bool {
    matches!(
        key,
        "scope" | "scope_id" | "storage_scope" | "object_scope" | "redis_namespace"
    )
}

fn replace_logical(value: &str, tenant: &str) -> String {
    if !value.contains(tenant) {
        return value.to_owned();
    }
    let mut result = String::with_capacity(value.len());
    let mut offset = 0;
    while let Some(found) = value[offset..].find(tenant) {
        let start = offset + found;
        let end = start + tenant.len();
        let before = value[..start].chars().next_back();
        let after = value[end..].chars().next();
        let boundary = |character: Option<char>| {
            character
                .is_none_or(|value| !value.is_ascii_alphanumeric() && value != '_' && value != '-')
        };
        if boundary(before) && boundary(after) {
            result.push_str(&value[offset..start]);
            result.push_str("<logical-tenant>");
            offset = end;
        } else {
            result.push_str(&value[offset..end]);
            offset = end;
        }
    }
    result.push_str(&value[offset..]);
    result
}

fn casefold(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for character in value.to_lowercase().chars() {
        match character {
            'ß' | 'ẞ' => result.push_str("ss"),
            'ς' => result.push('σ'),
            'ﬀ' => result.push_str("ff"),
            'ﬁ' => result.push_str("fi"),
            'ﬂ' => result.push_str("fl"),
            _ => result.push(character),
        }
    }
    result
}
