use super::ColumnInfo;
use crate::ValueType;

pub(super) fn literal_value(value: String, kind: ValueType) -> toml::Value {
    if kind == ValueType::Bool {
        match value.as_str() {
            "0" | "false" => return toml::Value::Boolean(false),
            "1" | "true" => return toml::Value::Boolean(true),
            _ => {}
        }
    }
    if kind.integer_bounds().is_some() && kind != ValueType::U64 {
        return value
            .parse::<i64>()
            .map(toml::Value::Integer)
            .unwrap_or(toml::Value::String(value));
    }
    toml::Value::String(value)
}

pub(super) struct Defaults {
    pub literal: Option<String>,
    pub expression: Option<String>,
    pub on_update: Option<String>,
}

pub(super) fn from_column(column: &ColumnInfo) -> Result<Defaults, String> {
    let extra = column.extra.to_ascii_lowercase();
    let generated_default = extra
        .split_whitespace()
        .any(|part| part == "default_generated");
    let remainder = extra
        .replace("default_generated", "")
        .replace("auto_increment", "");
    let remainder = remainder.trim();
    let on_update = if remainder.is_empty() {
        None
    } else {
        Some(
            remainder
                .strip_prefix("on update ")
                .ok_or_else(|| {
                    format!(
                        "列 {} 的 EXTRA `{}` 未支持；请审阅列生成或存储规则，导入未写入源码",
                        column.name, column.extra,
                    )
                })?
                .to_owned(),
        )
    };
    if generated_default && column.default.is_none() {
        return Err(format!(
            "列 {} 标记 DEFAULT_GENERATED 却缺少表达式；重新读取完整结构，导入未写入源码",
            column.name
        ));
    }
    Ok(Defaults {
        literal: (!generated_default)
            .then(|| column.default.clone())
            .flatten(),
        expression: generated_default.then(|| column.default.clone()).flatten(),
        on_update,
    })
}
