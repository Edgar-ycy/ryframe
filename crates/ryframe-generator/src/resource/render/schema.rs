use super::super::{FieldIr, ValueType};

/// DDL 与复制目录共用列类型，避免同一字段生成两种结构。
pub(super) fn column_type(field: &FieldIr) -> String {
    match field.value_type {
        ValueType::String => {
            let enum_length = field
                .enum_values
                .keys()
                .map(|value| u32::try_from(value.chars().count()).unwrap_or(u32::MAX))
                .max();
            let inferred = if field.name == "tenant_id" {
                Some(64)
            } else {
                enum_length
            };
            let length = field
                .validation
                .max_length
                .or(inferred)
                .unwrap_or(255)
                .max(1);
            format!("VARCHAR({length})")
        }
        ValueType::I32 => "INT".into(),
        ValueType::I64 => "BIGINT".into(),
        ValueType::Bool => "BOOLEAN".into(),
        ValueType::Date => "DATE".into(),
        ValueType::DateTime => "DATETIME(6)".into(),
        ValueType::Json => "JSON".into(),
        ValueType::Decimal => unreachable!("decimal 已在生成边界校验中拒绝"),
    }
}

pub(super) fn data_type(value: ValueType) -> &'static str {
    match value {
        ValueType::String => "varchar",
        ValueType::I32 => "int",
        ValueType::I64 => "bigint",
        ValueType::Bool => "tinyint",
        ValueType::Date => "date",
        ValueType::DateTime => "datetime",
        ValueType::Json => "json",
        ValueType::Decimal => unreachable!("decimal 已在生成边界校验中拒绝"),
    }
}
