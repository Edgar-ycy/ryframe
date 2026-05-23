/// 数据库类型 → Rust 类型映射
pub fn db_to_rust(db_type: &str, is_nullable: bool) -> String {
    let base = match db_type.to_lowercase().as_str() {
        "varchar" | "char" | "text" | "longtext" | "mediumtext" | "tinytext" | "uuid" => "String",
        "int" | "integer" | "int4" => "i32",
        "bigint" | "int8" => "i64",
        "smallint" | "int2" => "i16",
        "tinyint" => "i8",
        "boolean" | "bool" => "bool",
        "decimal" | "numeric" => "rust_decimal::Decimal",
        "float" | "float4" => "f32",
        "double" | "float8" => "f64",
        "timestamp" | "timestamptz" | "datetime" => "DateTime<Utc>",
        "date" => "chrono::NaiveDate",
        "json" | "jsonb" => "serde_json::Value",
        "bytea" | "blob" => "Vec<u8>",
        _ => "String",
    };
    if is_nullable {
        format!("Option<{}>", base)
    } else {
        base.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_to_rust_types() {
        assert_eq!(db_to_rust("int", false), "i32");
        assert_eq!(db_to_rust("bigint", false), "i64");
        assert_eq!(db_to_rust("varchar", false), "String");
        assert_eq!(db_to_rust("float", false), "f32");
        assert_eq!(db_to_rust("boolean", false), "bool");
        assert_eq!(db_to_rust("timestamp", false), "DateTime<Utc>");
        assert_eq!(db_to_rust("json", false), "serde_json::Value");
        assert_eq!(db_to_rust("custom_type", false), "String");
    }

    #[test]
    fn test_nullable_types() {
        assert_eq!(db_to_rust("varchar", true), "Option<String>");
        assert_eq!(db_to_rust("int", true), "Option<i32>");
    }
}
