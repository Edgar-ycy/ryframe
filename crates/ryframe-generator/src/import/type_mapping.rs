/// 保留 MySQL 有符号范围与精确类型；未知类型必须由调用方显式处理。
pub fn mysql_rust_type(column_type: &str, is_nullable: bool) -> Result<String, String> {
    let normalized = column_type.trim().to_ascii_lowercase();
    let data_type = normalized.split(['(', ' ']).next().unwrap_or_default();
    let unsigned = normalized.split_whitespace().any(|part| part == "unsigned");
    let base = match (data_type, unsigned) {
        ("varchar" | "char" | "text" | "longtext" | "mediumtext" | "tinytext", false) => "String",
        ("int" | "integer" | "mediumint", false) => "i32",
        ("int" | "integer" | "mediumint", true) => "u32",
        ("bigint", false) => "i64",
        ("bigint", true) => "u64",
        ("smallint", false) => "i16",
        ("smallint", true) => "u16",
        ("tinyint", false) => "i8",
        ("tinyint", true) => "u8",
        ("boolean" | "bool", false) => "bool",
        ("decimal" | "numeric", _) => "ryframe_sdk::rust_decimal::Decimal",
        ("float", _) => "f32",
        ("double", _) => "f64",
        ("datetime" | "timestamp", false) => {
            "ryframe_sdk::chrono::DateTime<ryframe_sdk::chrono::Utc>"
        }
        ("date", false) => "ryframe_sdk::time::Date",
        ("json", false) => "ryframe_sdk::serde_json::Value",
        ("binary" | "varbinary" | "tinyblob" | "blob" | "mediumblob" | "longblob", false) => {
            "Vec<u8>"
        }
        _ => {
            return Err(format!(
                "不支持 MySQL 类型 {column_type:?}；请显式转换为受支持字段，或实现强类型业务扩展"
            ));
        }
    };
    if is_nullable {
        Ok(format!("Option<{base}>"))
    } else {
        Ok(base.to_owned())
    }
}
