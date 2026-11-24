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
fn date_fields_preserve_string_schema_calendar_and_null() {
    use invoice_business::generated::dto::invoice::{
        CreateInvoiceDto, InvoiceListQuery, InvoiceVo, UpdateInvoiceDto,
    };
    use invoice_business::generated::models::invoice::InvoiceRecord;
    use ryframe_sdk::utoipa::{IntoParams, PartialSchema};
    use ryframe_sdk::{
        PaginationPolicy,
        axum::extract::Query,
        time::{Date, Month, OffsetDateTime},
    };

    let date = Date::from_calendar_date(2024, Month::February, 9).unwrap();
    let input: CreateInvoiceDto = serde_json::from_value(json!({
        "code": "first",
        "starts_on": "2024-2-9",
        "starts_at": "2024-02-09T00:00:00Z"
    }))
    .unwrap();
    assert_eq!(input.starts_on, date);
    assert_eq!(input.ends_on, None);
    let update: UpdateInvoiceDto =
        serde_json::from_value(json!({"starts_on":"2024-02-09","ends_on":null})).unwrap();
    assert_eq!(update.starts_on, Some(date));
    assert_eq!(update.ends_on, None);
    let historical: UpdateInvoiceDto =
        serde_json::from_value(json!({"ends_on":"0001-1-2"})).unwrap();
    assert_eq!(
        historical.ends_on,
        Some(Date::from_calendar_date(1, Month::January, 2).unwrap())
    );
    let Query(query) =
        Query::<InvoiceListQuery>::try_from_uri(&"/?starts_on=2024-2-9".parse().unwrap()).unwrap();
    assert_eq!(
        query
            .into_service_params(PaginationPolicy::new(20, 100))
            .unwrap()
            .starts_on,
        Some(date)
    );
    assert!(
        serde_json::from_value::<CreateInvoiceDto>(
            json!({"code":"missing","starts_at":"2024-02-09T00:00:00Z"})
        )
        .is_err()
    );
    for invalid in [json!("2023-2-29"), json!("10000-2-9"), json!([2024, 40])] {
        assert!(serde_json::from_value::<UpdateInvoiceDto>(json!({"ends_on":invalid})).is_err());
    }
    assert!(
        Query::<InvoiceListQuery>::try_from_uri(&"/?starts_on=2024-02-30".parse().unwrap())
            .is_err()
    );

    let timestamp = OffsetDateTime::UNIX_EPOCH;
    let view = InvoiceVo::from(InvoiceRecord {
        id: 1,
        tenant_id: "tenant-example".into(),
        code: "first".into(),
        memo: None,
        amount: 0,
        owner_id: 2,
        starts_on: date,
        ends_on: None,
        starts_at: timestamp,
        ends_at: None,
        created_at: timestamp,
        updated_at: timestamp,
    });
    let body = serde_json::to_value(view).unwrap();
    assert_eq!(body["starts_on"], "2024-02-09");
    assert!(body["ends_on"].is_null());

    let schema = serde_json::to_value(CreateInvoiceDto::schema()).unwrap();
    assert_eq!(schema["properties"]["starts_on"]["type"], "string");
    assert_eq!(schema["properties"]["starts_on"]["format"], "date");
    assert_eq!(
        schema["properties"]["ends_on"]["type"],
        json!(["string", "null"])
    );
    assert_eq!(schema["properties"]["ends_on"]["format"], "date");
    let parameters = serde_json::to_value(InvoiceListQuery::into_params(|| None)).unwrap();
    let date_parameter = parameters
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["name"] == "starts_on")
        .unwrap();
    assert_eq!(date_parameter["schema"]["format"], "date");
}

#[test]
fn datetime_keeps_utc_json_shapes_and_query_contract() {
    use invoice_business::generated::{
        dto::invoice::{CreateInvoiceDto, InvoiceListQuery, InvoiceVo, UpdateInvoiceDto},
        models::invoice::InvoiceRecord,
    };
    use ryframe_sdk::utoipa::{IntoParams, PartialSchema};
    use ryframe_sdk::{PaginationPolicy, axum::extract::Query, time::OffsetDateTime};

    let input: CreateInvoiceDto = serde_json::from_value(json!({
        "code": "timed",
        "starts_on": "2024-02-09",
        "starts_at": "2024-02-09T12:00:00.123456789+08:00",
        "ends_at": null
    }))
    .unwrap();
    assert_eq!(input.starts_at.offset(), ryframe_sdk::time::UtcOffset::UTC);
    assert_eq!(input.starts_at.hour(), 4);
    assert_eq!(input.starts_at.nanosecond(), 123_456_789);
    assert_eq!(input.ends_at, None);
    let missing: UpdateInvoiceDto = serde_json::from_value(json!({})).unwrap();
    assert_eq!(missing.ends_at, None);
    assert!(
        serde_json::from_value::<CreateInvoiceDto>(
            json!({"code":"missing","starts_on":"2024-02-09"})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<UpdateInvoiceDto>(json!({"ends_at":"2024-02-09T12:00:00"}))
            .is_err()
    );
    let Query(query) = Query::<InvoiceListQuery>::try_from_uri(
        &"/?starts_at=2024-02-09T12:00:00.123456789%2B08:00"
            .parse()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        query
            .into_service_params(PaginationPolicy::new(20, 100))
            .unwrap()
            .starts_at,
        Some(input.starts_at)
    );

    let view = InvoiceVo::from(InvoiceRecord {
        id: 1,
        tenant_id: "tenant-example".into(),
        code: "timed".into(),
        memo: None,
        amount: 0,
        owner_id: 2,
        starts_on: input.starts_on,
        ends_on: None,
        starts_at: input.starts_at,
        ends_at: None,
        created_at: OffsetDateTime::UNIX_EPOCH,
        updated_at: OffsetDateTime::UNIX_EPOCH
            .replace_nanosecond(123_456_000)
            .unwrap(),
    });
    let body = serde_json::to_value(view).unwrap();
    assert_eq!(body["starts_at"], "2024-02-09T04:00:00.123456789Z");
    assert!(body["ends_at"].is_null());
    assert_eq!(body["updated_at"], "1970-01-01T00:00:00.123456Z");

    let schema = serde_json::to_value(CreateInvoiceDto::schema()).unwrap();
    assert_eq!(schema["properties"]["starts_at"]["type"], "string");
    assert_eq!(schema["properties"]["starts_at"]["format"], "date-time");
    assert_eq!(
        schema["properties"]["ends_at"]["type"],
        json!(["string", "null"])
    );
    assert_eq!(schema["properties"]["ends_at"]["format"], "date-time");
    let parameters = serde_json::to_value(InvoiceListQuery::into_params(|| None)).unwrap();
    let parameter = parameters
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["name"] == "starts_at")
        .unwrap();
    assert_eq!(parameter["schema"]["format"], "date-time");
}

#[test]
fn imported_integer_dtos_accept_boundaries_and_reject_overflow() {
    use invoice_business::generated::dto::memo::{CreateMemoDto, UpdateMemoDto};
    let body = json!({"code":"first", "status":1, "tiny_count":-128, "small_count":-32768, "byte_count":255, "short_count":65535, "count":4294967295u32, "large_count":"18446744073709551615"});
    let input: CreateMemoDto = serde_json::from_value(body.clone()).unwrap();
    input.validate().unwrap();
    assert_eq!(input.large_count, Some(u64::MAX));
    let mut missing_default = body.clone();
    missing_default
        .as_object_mut()
        .unwrap()
        .remove("large_count");
    let missing_default: CreateMemoDto = serde_json::from_value(missing_default).unwrap();
    missing_default.validate().unwrap();
    assert!(missing_default.large_count.is_none());
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
fn unsigned_body_schema_and_query_preserve_maximum() {
    use invoice_business::generated::dto::memo_item::{MemoItemListQuery, UpdateMemoItemDto};
    let input: UpdateMemoItemDto =
        serde_json::from_value(json!({"large_count":u64::MAX.to_string()})).unwrap();
    input.validate().unwrap();
    assert_eq!(input.large_count, Some(u64::MAX));
    let query: MemoItemListQuery =
        serde_json::from_value(json!({"large_count":u64::MAX.to_string()})).unwrap();
    assert_eq!(
        query
            .into_service_params(ryframe_sdk::PaginationPolicy::new(20, 100))
            .unwrap()
            .large_count,
        Some(u64::MAX)
    );
    let schema =
        serde_json::to_value(<UpdateMemoItemDto as ryframe_sdk::utoipa::PartialSchema>::schema())
            .unwrap();
    assert_eq!(
        schema["properties"]["large_count"]["type"],
        json!(["string", "null"])
    );
    assert_eq!(
        schema["properties"]["large_count"]["pattern"],
        "^(?:0|[1-9][0-9]*)$"
    );
    for value in [
        json!(u64::MAX),
        json!("-1"),
        json!("18446744073709551616"),
        json!("01"),
    ] {
        assert!(serde_json::from_value::<UpdateMemoItemDto>(json!({"large_count":value})).is_err());
    }
    for value in ["-1", "18446744073709551616", "01"] {
        let query: MemoItemListQuery =
            serde_json::from_value(json!({"large_count":value})).unwrap();
        assert!(
            query
                .into_service_params(ryframe_sdk::PaginationPolicy::new(20, 100))
                .is_err()
        );
    }
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
