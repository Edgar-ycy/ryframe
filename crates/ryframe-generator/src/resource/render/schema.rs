use super::super::{FieldIr, ValueType};

/// DDL 与复制目录共用列类型，避免同一字段生成两种结构。
pub(crate) fn column_type(field: &FieldIr) -> String {
    if let Some(declaration) = &field.sql_type {
        return declaration.clone();
    }
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
        ValueType::I8 => "TINYINT".into(),
        ValueType::I16 => "SMALLINT".into(),
        ValueType::I32 => "INT".into(),
        ValueType::I64 => "BIGINT".into(),
        ValueType::U8 => "TINYINT UNSIGNED".into(),
        ValueType::U16 => "SMALLINT UNSIGNED".into(),
        ValueType::U32 => "INT UNSIGNED".into(),
        ValueType::U64 => "BIGINT UNSIGNED".into(),
        ValueType::Bool => "BOOLEAN".into(),
        ValueType::Date => "DATE".into(),
        ValueType::DateTime => "DATETIME(6)".into(),
        ValueType::Json => "JSON".into(),
        ValueType::Decimal => unreachable!("Decimal 已在生成边界要求显式 SQL 类型"),
    }
}

pub(super) fn data_type(field: &FieldIr) -> &str {
    if let Some(declaration) = &field.sql_type {
        return match declaration
            .split(['(', ' '])
            .next()
            .expect("SQL 类型已校验")
        {
            "CHAR" => "char",
            "VARCHAR" => "varchar",
            "TINYTEXT" => "tinytext",
            "TEXT" => "text",
            "MEDIUMTEXT" => "mediumtext",
            "LONGTEXT" => "longtext",
            "TINYINT" => "tinyint",
            "SMALLINT" => "smallint",
            "MEDIUMINT" => "mediumint",
            "INT" => "int",
            "BIGINT" => "bigint",
            "DATE" => "date",
            "DATETIME" => "datetime",
            "JSON" => "json",
            "DECIMAL" => "decimal",
            _ => unreachable!("SQL 类型已校验"),
        };
    }
    match field.value_type {
        ValueType::String => "varchar",
        ValueType::I8 | ValueType::U8 => "tinyint",
        ValueType::I16 | ValueType::U16 => "smallint",
        ValueType::I32 | ValueType::U32 => "int",
        ValueType::I64 | ValueType::U64 => "bigint",
        ValueType::Bool => "tinyint",
        ValueType::Date => "date",
        ValueType::DateTime => "datetime",
        ValueType::Json => "json",
        ValueType::Decimal => unreachable!("Decimal 已在生成边界要求显式 SQL 类型"),
    }
}
