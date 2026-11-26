#[test]
fn message_publish_expiry_accepts_optional_rfc3339_and_preserves_schema() {
    use ryframe_api::dto::message_dto::PublishMessageDto;
    use serde_json::{Value, json};
    use time::{UtcOffset, macros::datetime};
    use utoipa::PartialSchema;

    let body = json!({
        "topic": "fixture",
        "title": "测试消息",
        "content": "消息正文",
        "severity": "info",
        "audiences": [{"kind": "tenant"}]
    });
    let parse = |expiry: Value| {
        let mut body = body.clone();
        body["expires_at"] = expiry;
        serde_json::from_value::<PublishMessageDto>(body).map(|dto| dto.expires_at)
    };

    let omitted = serde_json::from_value::<PublishMessageDto>(body.clone()).unwrap();
    assert_eq!(omitted.expires_at, None);
    assert_eq!(parse(Value::Null).unwrap(), None);

    for (input, expected) in [
        (
            "1970-01-01T07:59:59.123456789+08:00",
            datetime!(1969-12-31 23:59:59.123456789 UTC),
        ),
        ("0000-01-01T00:00:00Z", datetime!(0000-01-01 0:00 UTC)),
        (
            "9999-12-31T23:59:59.999999999Z",
            datetime!(9999-12-31 23:59:59.999999999 UTC),
        ),
    ] {
        let expiry = parse(json!(input)).unwrap().unwrap();
        assert_eq!(expiry, expected);
        assert_eq!(expiry.offset(), UtcOffset::UTC);
    }
    for invalid in [
        json!(123),
        json!("1969-12-31T23:59:59"),
        json!("2026-02-30T00:00:00Z"),
        json!("2016-12-31T23:59:60Z"),
        json!("10000-01-01T00:00:00Z"),
    ] {
        assert!(parse(invalid.clone()).is_err(), "{invalid}");
    }

    let schema = serde_json::to_value(PublishMessageDto::schema()).unwrap();
    let expiry = &schema["properties"]["expires_at"];
    assert_eq!(expiry["type"], json!(["string", "null"]));
    assert_eq!(expiry["format"], "date-time");
    let required = schema["required"].as_array().unwrap();
    assert!(!required.contains(&json!("expires_at")));
}
