use std::path::Path;

use ryframe_kernel::{AppError, AppResult};

pub(super) fn read_table(path: &Path) -> AppResult<toml::Table> {
    let source = std::fs::read_to_string(path).map_err(|error| {
        AppError::Config(format!(
            "无法读取配置文件 {}（{}）；请创建该文件并检查路径、读取权限及 UTF-8 编码",
            path.display(),
            error.kind()
        ))
    })?;
    toml::from_str(&source).map_err(|error: toml::de::Error| {
        let offset = error.span().map_or(0, |span| span.start).min(source.len());
        let prefix = &source.as_bytes()[..offset];
        let line = prefix.iter().filter(|byte| **byte == b'\n').count() + 1;
        let column = prefix.iter().rposition(|byte| *byte == b'\n').map_or(offset + 1, |last| offset - last);
        // 不输出解析器携带的原始行或值；位置用于定位出错的字段。
        AppError::Config(format!(
            "配置文件 {} 第 {line} 行第 {column} 列存在 TOML 语法错误；请检查该位置字段的引号、括号、重复键和字面量格式",
            path.display()
        ))
    })
}

pub(super) fn configuration_error(context: &str, table: &toml::Table, error: AppError) -> AppError {
    let message = match error {
        AppError::Config(message) => message,
        _ => "配置校验失败".to_owned(),
    };
    AppError::Config(format!(
        "配置文件 {context} 字段校验失败：{}；请修正对应 TOML 字段",
        redact_values(table, &message)
    ))
}

pub(super) fn redact_values(table: &toml::Table, message: &str) -> String {
    let mut values = Vec::new();
    for value in table.values() {
        collect_values(value, &mut values);
    }
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    values.dedup();
    let mut result = message.to_owned();
    for value in values {
        if !value.is_empty() {
            for literal in [
                format!("\"{value}\""),
                format!("`{value}`"),
                format!("'{value}'"),
            ] {
                result = result.replace(&literal, "<已隐藏>");
            }
            for prefix in [": ", "："] {
                result = result.replace(&format!("{prefix}{value}"), &format!("{prefix}<已隐藏>"));
            }
        }
    }
    result
}

fn collect_values(value: &toml::Value, values: &mut Vec<String>) {
    match value {
        toml::Value::Table(table) => {
            for child in table.values() {
                collect_values(child, values);
            }
        }
        toml::Value::Array(items) => {
            for child in items {
                collect_values(child, values);
            }
        }
        toml::Value::String(text) => {
            values.push(text.clone());
            let escaped = format!("{text:?}");
            values.push(escaped[1..escaped.len() - 1].to_owned());
        }
        _ => values.push(value.to_string()),
    }
}
