#[path = "../src/id_parser.rs"]
mod id_parser;

use id_parser::{parse_id, parse_optional_id, parse_optional_positive_id, parse_positive_id_list};
use ryframe_kernel::AppError;

#[test]
fn signed_i64_contract_is_preserved() {
    assert_eq!(parse_id("-1", "ID 无效").expect("负数仍是有效 i64"), -1);
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
    assert_eq!(parse_optional_id(Some("  "), "部门ID").unwrap(), None);
    let error = parse_optional_id(Some("abc"), "部门ID").expect_err("非整数应校验失败");
    assert!(matches!(
        error,
        AppError::Validation(message) if message == "无效的部门ID: abc"
    ));
}
