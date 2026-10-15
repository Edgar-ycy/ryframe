use ryframe_api::http::integer;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct Required {
    #[serde(deserialize_with = "integer::deserialize_i64")]
    value: i64,
}

#[derive(Debug, Deserialize)]
struct Optional {
    #[serde(default, deserialize_with = "integer::deserialize_optional_i64")]
    value: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct Unsigned {
    #[serde(deserialize_with = "integer::deserialize_u64")]
    value: u64,
}

#[derive(Debug, Deserialize)]
struct OptionalUnsigned {
    #[serde(default, deserialize_with = "integer::deserialize_optional_u64")]
    value: Option<u64>,
}

#[test]
fn unsigned_json_preserves_full_capacity_and_rejects_noncanonical_input() {
    for value in [0, 9_007_199_254_740_993, i64::MAX as u64 + 1, u64::MAX] {
        let body = json!({"value":value.to_string()});
        assert_eq!(
            serde_json::from_value::<Unsigned>(body.clone())
                .unwrap()
                .value,
            value
        );
        assert_eq!(
            serde_json::from_value::<OptionalUnsigned>(body)
                .unwrap()
                .value,
            Some(value)
        );
        assert_eq!(integer::parse_u64(&value.to_string()).unwrap(), value);
    }
    for body in [json!({}), json!({"value":null})] {
        assert!(serde_json::from_value::<Unsigned>(body.clone()).is_err());
        assert!(
            serde_json::from_value::<OptionalUnsigned>(body)
                .unwrap()
                .value
                .is_none()
        );
    }
    for body in [
        json!({"value":1}),
        json!({"value":u64::MAX}),
        json!({"value":1.5}),
        json!({"value":true}),
        json!({"value":[]}),
    ] {
        assert!(serde_json::from_value::<Unsigned>(body.clone()).is_err());
        assert!(serde_json::from_value::<OptionalUnsigned>(body).is_err());
    }
    for value in [
        "",
        "-1",
        "-0",
        "+1",
        "01",
        "1.0",
        "1e3",
        " 1",
        "1 ",
        "18446744073709551616",
    ] {
        assert!(integer::parse_u64(value).is_err(), "{value}");
        assert!(serde_json::from_value::<Unsigned>(json!({"value":value})).is_err());
        assert!(serde_json::from_value::<OptionalUnsigned>(json!({"value":value})).is_err());
    }
}

#[test]
fn signed_json_strings_keep_all_i64_bits_and_optional_semantics() {
    for value in [
        i64::MIN,
        -9_007_199_254_740_993,
        -1,
        0,
        1,
        9_007_199_254_740_993,
        i64::MAX,
    ] {
        let body = json!({"value":value.to_string()});
        assert_eq!(
            serde_json::from_value::<Required>(body.clone())
                .unwrap()
                .value,
            value
        );
        assert_eq!(
            serde_json::from_value::<Optional>(body).unwrap().value,
            Some(value)
        );
        assert_eq!(integer::parse_i64(&value.to_string()).unwrap(), value);
    }
    assert!(
        serde_json::from_value::<Optional>(json!({}))
            .unwrap()
            .value
            .is_none()
    );
    assert!(
        serde_json::from_value::<Optional>(json!({"value":null}))
            .unwrap()
            .value
            .is_none()
    );
    assert!(serde_json::from_value::<Required>(json!({})).is_err());
    assert!(serde_json::from_value::<Required>(json!({"value":null})).is_err());
}

#[test]
fn numeric_json_overflow_and_noncanonical_strings_fail_closed() {
    for body in [
        json!({"value":1}),
        json!({"value":1.5}),
        json!({"value":true}),
        json!({"value":[]}),
    ] {
        assert!(serde_json::from_value::<Required>(body.clone()).is_err());
        assert!(serde_json::from_value::<Optional>(body).is_err());
    }
    for value in [
        "",
        "01",
        "+1",
        "-0",
        "1.0",
        "1e3",
        " 1",
        "1 ",
        "9223372036854775808",
        "-9223372036854775809",
    ] {
        assert!(integer::parse_i64(value).is_err(), "{value}");
        assert!(serde_json::from_value::<Required>(json!({"value":value})).is_err());
        assert!(serde_json::from_value::<Optional>(json!({"value":value})).is_err());
    }
}
