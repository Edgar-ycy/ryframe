use ryframe_sdk::ResourceModel;

#[derive(ResourceModel)]
#[resource(name = "typed_model", table = "biz_typed_model", database = "control")]
pub struct TypedModel {
    #[resource(primary_key, auto_increment)]
    pub id: i64,
    pub payload: ryframe_sdk::serde_json::Value,
    pub date: ryframe_sdk::chrono::NaiveDate,
    pub amount: ryframe_sdk::rust_decimal::Decimal,
}

#[test]
fn model_types_compile_without_runtime_api_or_database_features() {
    let descriptor = TypedModel::descriptor();
    assert_eq!(descriptor.fields.len(), 4);
    assert!(descriptor.fields[0].auto_increment);
    assert!(descriptor.fields[0].generated);
    assert!(!descriptor.fields[1].auto_increment);
    assert!(descriptor.fields[1].rust_type.contains("serde_json"));
    let model = TypedModel {
        id: 1,
        payload: ryframe_sdk::serde_json::json!({"value": 1}),
        date: ryframe_sdk::chrono::NaiveDate::from_ymd_opt(2026, 10, 13).unwrap(),
        amount: ryframe_sdk::rust_decimal::Decimal::from_str_exact("12345678901234567890.123456")
            .unwrap(),
    };
    assert_eq!(model.amount.to_string(), "12345678901234567890.123456");
}
