#[path = "../../src/id_parser.rs"]
mod id_parser;

use id_parser::{parse_id, parse_optional_id, parse_optional_positive_id, parse_positive_id_list};
use ryframe_kernel::AppError;

#[test]
fn signed_i64_contract_is_preserved() {
    for (value, expected) in [
        ("-9223372036854775808", i64::MIN),
        ("9223372036854775807", i64::MAX),
        ("0", 0),
        ("-1", -1),
        ("-0", 0),
        ("+1", 1),
        ("01", 1),
    ] {
        assert_eq!(parse_id(value, "ID 无效").unwrap(), expected, "{value}");
    }
    for value in [
        "9223372036854775808",
        "-9223372036854775809",
        "",
        " ",
        " 1",
        "1 ",
        "1.0",
        "1e0",
        "--1",
        "abc",
    ] {
        assert!(
            matches!(parse_id(value, "ID 无效"), Err(AppError::Validation(actual)) if actual == "ID 无效"),
            "{value:?}"
        );
    }
}

#[test]
fn unsigned_id_keeps_all_bits_and_supplied_error_message() {
    for value in [0, i64::MAX as u64 + 1, u64::MAX] {
        assert_eq!(
            ryframe_api::parse_unsigned_id(&value.to_string(), "业务 ID 无效").unwrap(),
            value
        );
    }
    for value in ["-1", "-0", "01", "+1", "1.0", " 1", "18446744073709551616"] {
        assert!(
            matches!(id_parser::parse_unsigned_id(value, "业务 ID 无效"), Err(AppError::Validation(message)) if message == "业务 ID 无效")
        );
    }
}

#[test]
fn positive_id_helpers_preserve_the_supplied_message() {
    let error =
        parse_optional_positive_id(Some("0"), "任务 ID 必须是正整数").expect_err("零不是正整数 ID");
    assert!(matches!(
        error,
        AppError::Validation(message) if message == "任务 ID 必须是正整数"
    ));

    let ids = vec!["1".to_owned(), "2".to_owned()];
    assert_eq!(
        parse_positive_id_list(&ids, "任务 ID 必须是正整数").expect("列表应解析成功"),
        vec![1, 2]
    );
}

#[test]
fn optional_id_preserves_empty_and_error_contracts() {
    for value in [None, Some(""), Some("  "), Some("\t\r\n")] {
        assert_eq!(parse_optional_id(value, "部门ID").unwrap(), None);
    }
    for (value, expected) in [
        (" -9223372036854775808 ", i64::MIN),
        ("\t9223372036854775807\n", i64::MAX),
        (" 0 ", 0),
        (" -1 ", -1),
        (" +1 ", 1),
        (" 01 ", 1),
    ] {
        assert_eq!(
            parse_optional_id(Some(value), "部门ID").unwrap(),
            Some(expected)
        );
    }
    for value in [
        "9223372036854775808",
        "-9223372036854775809",
        " abc ",
        "1.0",
        "1 2",
    ] {
        assert!(
            matches!(parse_optional_id(Some(value), "部门ID"),
                Err(AppError::Validation(message)) if message == format!("无效的部门ID: {}", value.trim())),
            "{value:?}"
        );
    }
}
