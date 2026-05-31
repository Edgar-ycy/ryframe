use ryframe_generator::type_mapping::db_to_rust;

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
