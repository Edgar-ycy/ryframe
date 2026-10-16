use ryframe_sdk::ResourceModel;

#[derive(ResourceModel)]
#[resource(
    name = "typed_model",
    table = "biz_typed_model",
    database = "control",
    frontend_page = "@/views/custom/index.vue"
)]
pub struct TypedModel {
    #[resource(primary_key, auto_increment)]
    pub id: i64,
    #[resource(sql_comment = "中文 '注释'\\路径")]
    pub payload: ryframe_sdk::serde_json::Value,
    pub date: ryframe_sdk::chrono::NaiveDate,
    pub amount: ryframe_sdk::rust_decimal::Decimal,
}

#[test]
fn model_types_compile_without_runtime_api_or_database_features() {
    let descriptor = TypedModel::descriptor();
    assert_eq!(descriptor.fields.len(), 4);
    assert_eq!(descriptor.frontend_page, Some("@/views/custom/index.vue"));
    assert_eq!(descriptor.primary_key, ["id"]);
    assert!(descriptor.fields[0].auto_increment);
    assert!(descriptor.fields[0].generated);
    assert!(!descriptor.fields[1].auto_increment);
    assert!(descriptor.fields[1].rust_type.contains("serde_json"));
    assert_eq!(descriptor.fields[1].sql_comment, Some("中文 '注释'\\路径"));
    let model = TypedModel {
        id: 1,
        payload: ryframe_sdk::serde_json::json!({"value": 1}),
        date: ryframe_sdk::chrono::NaiveDate::from_ymd_opt(2026, 10, 13).unwrap(),
        amount: ryframe_sdk::rust_decimal::Decimal::from_str_exact("12345678901234567890.123456")
            .unwrap(),
    };
    assert_eq!(model.amount.to_string(), "12345678901234567890.123456");
}

#[derive(ResourceModel)]
#[resource(name = "ordered", table = "biz_ordered", database = "tenant", primary_key = ["tenant_id", "id"])]
pub struct OrderedModel {
    #[resource(primary_key, auto_increment, column = "order_id")]
    pub id: i64,
    pub code: String,
    #[resource(primary_key)]
    pub tenant_id: String,
}

#[test]
fn explicit_key_order_is_independent_of_model_field_and_physical_column_order() {
    let descriptor = OrderedModel::descriptor();
    assert_eq!(descriptor.frontend_page, None);
    assert_eq!(descriptor.primary_key, ["tenant_id", "id"]);
    assert_eq!(
        descriptor
            .fields
            .iter()
            .map(|field| field.column)
            .collect::<Vec<_>>(),
        ["order_id", "code", "tenant_id"]
    );
}
