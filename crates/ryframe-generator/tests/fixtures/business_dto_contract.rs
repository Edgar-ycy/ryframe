use invoice_business::generated::dto::global_item::{CreateGlobalItemDto, UpdateGlobalItemDto};
use ryframe_sdk::serde_json::{self, json};
use ryframe_sdk::validator::Validate;

#[test]
fn nullable_values_follow_complete_replacement_contract() {
    let missing: CreateGlobalItemDto = serde_json::from_value(json!({"code":"first"})).unwrap();
    assert!(missing.memo.is_none());
    let empty: UpdateGlobalItemDto = serde_json::from_value(json!({"memo":null})).unwrap();
    assert!(empty.memo.is_none());
    let value: UpdateGlobalItemDto = serde_json::from_value(json!({"memo":"text"})).unwrap();
    assert_eq!(value.memo.as_deref(), Some("text"));
    let omitted: UpdateGlobalItemDto = serde_json::from_value(json!({})).unwrap();
    assert!(omitted.memo.is_none());
}

#[test]
fn imported_integer_dtos_accept_boundaries_and_reject_overflow() {
    use invoice_business::generated::dto::memo::{CreateMemoDto, UpdateMemoDto};
    let body = json!({"code":"first", "status":1, "tiny_count":-128, "small_count":-32768, "byte_count":255, "short_count":65535, "count":4294967295u32});
    let input: CreateMemoDto = serde_json::from_value(body.clone()).unwrap();
    input.validate().unwrap();
    assert_eq!(
        (
            input.tiny_count,
            input.small_count,
            input.byte_count,
            input.short_count,
            input.count
        ),
        (-128, -32768, 255, 65535, u32::MAX)
    );
    for (field, invalid) in [
        ("tiny_count", -129i64),
        ("small_count", 32768),
        ("byte_count", -1),
        ("short_count", 65536),
        ("count", 4294967296),
    ] {
        let mut bad = body.clone();
        bad[field] = json!(invalid);
        assert!(
            serde_json::from_value::<CreateMemoDto>(bad).is_err(),
            "{field}"
        );
    }
    let update: UpdateMemoDto = serde_json::from_value(json!({"count":4294967295u32})).unwrap();
    update.validate().unwrap();
    assert_eq!(update.count, Some(u32::MAX));
}

#[test]
fn signed_i64_body_schema_and_query_keep_decimal_strings_exact() {
    use invoice_business::generated::dto::global_item::GlobalItemListQuery;
    for value in [
        i64::MIN,
        -9_007_199_254_740_993,
        9_007_199_254_740_993,
        i64::MAX,
    ] {
        let input: CreateGlobalItemDto =
            serde_json::from_value(json!({"code":"exact", "amount":value.to_string()})).unwrap();
        input.validate().unwrap();
        assert_eq!(input.amount, Some(value));
        let update: UpdateGlobalItemDto =
            serde_json::from_value(json!({"amount":value.to_string()})).unwrap();
        update.validate().unwrap();
        assert_eq!(update.amount, Some(value));
        let query: GlobalItemListQuery =
            serde_json::from_value(json!({"amount":value.to_string()})).unwrap();
        let params = query
            .into_service_params(ryframe_sdk::PaginationPolicy::new(20, 100))
            .unwrap();
        assert_eq!(params.amount, Some(value));
    }
    assert!(
        serde_json::from_value::<CreateGlobalItemDto>(
            json!({"code":"invalid","amount":9007199254740993i64})
        )
        .is_err()
    );
    let schema =
        serde_json::to_value(<CreateGlobalItemDto as ryframe_sdk::utoipa::PartialSchema>::schema())
            .unwrap();
    assert_eq!(
        schema["properties"]["amount"]["type"],
        json!(["string", "null"])
    );
    assert_eq!(
        schema["properties"]["amount"]["pattern"],
        "^(?:0|-?[1-9][0-9]*)$"
    );
    for invalid in ["+1", "01", "-0", "9223372036854775808"] {
        let query: GlobalItemListQuery = serde_json::from_value(json!({"amount":invalid})).unwrap();
        assert!(
            query
                .into_service_params(ryframe_sdk::PaginationPolicy::new(20, 100))
                .is_err()
        );
    }
}
