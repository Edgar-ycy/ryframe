use ryframe_generator::{
    naming::{to_camel_case, to_pascal_case, to_snake_case},
    type_mapping::db_to_rust,
};

#[test]
fn naming_conversions_are_stable() {
    assert_eq!(to_snake_case("UserName"), "user_name");
    assert_eq!(to_pascal_case("user_name"), "UserName");
    assert_eq!(to_camel_case("user_name"), "userName");
    assert_eq!(to_camel_case("UserName"), "userName");
}

#[test]
fn database_types_map_to_rust_types() {
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
fn nullable_database_types_become_options() {
    assert_eq!(db_to_rust("varchar", true), "Option<String>");
    assert_eq!(db_to_rust("int", true), "Option<i32>");
}
