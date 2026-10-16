use rust_decimal::Decimal;
use ryframe_api::http::decimal;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct Required {
    #[serde(deserialize_with = "decimal::deserialize_decimal")]
    value: Decimal,
}

#[derive(Debug, Deserialize)]
struct Optional {
    #[serde(default, deserialize_with = "decimal::deserialize_optional_decimal")]
    value: Option<Decimal>,
}

#[test]
fn decimal_strings_preserve_all_digits_and_optional_semantics() {
    for value in [
        "0",
        "0.125000",
        "9007199254740993.125000",
        "9999999999999999999999.999999",
        "-9999999999999999999999.999999",
        "0.0000000000000000000000000001",
    ] {
        let body = json!({"value":value});
        let required: Required = serde_json::from_value(body.clone()).unwrap();
        let optional: Optional = serde_json::from_value(body).unwrap();
        assert_eq!(required.value.to_string(), value);
        assert_eq!(optional.value.unwrap(), required.value);
    }
    for body in [json!({}), json!({"value":null})] {
        assert!(serde_json::from_value::<Required>(body.clone()).is_err());
        assert!(
            serde_json::from_value::<Optional>(body)
                .unwrap()
                .value
                .is_none()
        );
    }
    for value in [json!(1), json!(0.125), json!(true), json!([])] {
        let body = json!({"value":value});
        assert!(serde_json::from_value::<Required>(body.clone()).is_err());
        assert!(serde_json::from_value::<Optional>(body).is_err());
    }
    for value in [
        "",
        "1e3",
        "+1",
        "01",
        "-0",
        "-0.0000",
        ".5",
        "1.",
        "1_000",
        " 1",
        "1 ",
        "NaN",
        "999999999999999999999999999999",
        "0.00000000000000000000000000001",
    ] {
        assert!(decimal::parse_decimal(value).is_err(), "{value}");
        assert!(serde_json::from_value::<Required>(json!({"value":value})).is_err());
    }
}

#[test]
fn sql_capacity_includes_zero_and_fraction_only_domains_without_rounding() {
    for (value, precision, scale, valid) in [
        ("0", 8, 8, true),
        ("0.00000001", 8, 8, true),
        ("-0.00000001", 8, 8, true),
        ("1", 8, 8, false),
        ("9.99", 3, 2, true),
        ("10.00", 3, 2, false),
        ("1.234", 3, 2, false),
        ("1.230", 3, 2, false),
        ("9999999999999999999999999999", 28, 0, true),
        ("10000000000000000000000000000", 28, 0, false),
        ("0", 0, 0, false),
        ("0", 29, 0, false),
        ("0", 20, 21, false),
    ] {
        let parsed = decimal::parse_decimal(value).unwrap();
        assert_eq!(
            ryframe_kernel::decimal_fits_precision(parsed, precision, scale),
            valid,
            "{value}/{precision}/{scale}"
        );
        assert_eq!(
            decimal::parse_decimal_with_capacity(value, precision, scale).is_ok(),
            valid
        );
    }
}
