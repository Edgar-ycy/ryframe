use super::{FieldSpec, ResourceError, ValueType};

/// 数据库时钟表达式仅接受确定的类型及精度，不拼接任意 SQL。
pub(super) fn normalize_fields(
    fields: &mut [FieldSpec],
    resource: &str,
    path: &str,
) -> Result<(), ResourceError> {
    for field in fields {
        if field.sql_default.is_none() && field.sql_on_update.is_none() {
            continue;
        }
        let invalid = |reason: &str| {
            ResourceError::new(
            format!("数据库默认值或更新表达式无效：{reason}；本批未写入"),
            "DATETIME 仅支持与列精度一致的 CURRENT_TIMESTAMP 或 CURRENT_TIMESTAMP(0..6)；字面量使用 default，其他表达式使用明确的业务迁移扩展",
        ).with_resource(resource).with_file(path).with_field(&field.name)
        };
        if field.value_type != ValueType::DateTime {
            return Err(invalid("当前仅支持 DATETIME 数据库时钟表达式"));
        }
        if field.default.is_some() && field.sql_default.is_some() {
            return Err(invalid("default 与 sql_default 不能同时声明"));
        }
        let precision = match field.sql_type.as_deref() {
            None => 6,
            Some("DATETIME") => 0,
            Some(value) => value
                .strip_prefix("DATETIME(")
                .and_then(|value| value.strip_suffix(')'))
                .and_then(|value| value.parse::<u8>().ok())
                .ok_or_else(|| invalid("不是已校验的 DATETIME 类型"))?,
        };
        let normalize = |value: &str| {
            let value = value.trim().to_ascii_uppercase();
            let expected = if precision == 0 {
                "CURRENT_TIMESTAMP".into()
            } else {
                format!("CURRENT_TIMESTAMP({precision})")
            };
            if value == expected || (precision == 0 && value == "CURRENT_TIMESTAMP(0)") {
                Ok(expected)
            } else {
                Err(invalid("表达式未支持或与 DATETIME 列精度不一致"))
            }
        };
        field.sql_default = field.sql_default.as_deref().map(normalize).transpose()?;
        field.sql_on_update = field.sql_on_update.as_deref().map(normalize).transpose()?;
    }
    Ok(())
}
