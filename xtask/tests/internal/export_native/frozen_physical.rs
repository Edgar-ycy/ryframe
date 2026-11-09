use serde_json::json;

use super::*;

#[test]
fn full_fold_preserves_greek_code_point_order_and_uses_default_mappings() {
    let table = FOLDING.as_ref().unwrap();
    assert_eq!(fold("\u{0390}", table), "\u{03b9}\u{0308}\u{0301}");
    assert_eq!(fold("\u{1f52}", table), "\u{03c5}\u{0313}\u{0300}");
    assert_eq!(fold("I\u{0130}\u{0131}", table), "ii\u{0307}\u{0131}");
    assert_eq!(fold("\u{fb03}\u{1f980}", table), "ffi\u{1f980}");
    assert_eq!(fold("EXAMPLE", table), "example");
}

#[test]
fn raw_non_ascii_endpoint_matches_the_same_folding_without_reordering() {
    let logical = BTreeSet::new();
    let check = Check::new(
        &["http://127.0.0.1:39011/CAFÉ/\u{0390}/../..".into()],
        &logical,
    )
    .unwrap();
    let row = |endpoint: &str| Row::from([("body".into(), SqlValue::Text(endpoint.into()))]);
    assert!(
        check
            .row(&row(
                "http://127.0.0.1:39011/café/\u{03b9}\u{0308}\u{0301}/../.."
            ))
            .is_err()
    );
    check
        .row(&row(
            "http://127.0.0.1:39011/café/\u{03b9}\u{0301}\u{0308}/../..",
        ))
        .unwrap();
}

#[test]
fn physical_scan_checks_root_keys_bytes_and_nested_dictionary_exemptions() {
    let logical = BTreeSet::from(["alpha-tenant".to_owned()]);
    let check = Check::new(&["alpha".into(), "tenant_id".into()], &logical).unwrap();
    let root = |key: &str, value| Row::from([(key.into(), value)]);
    assert!(check.row(&root("tenant_id", SqlValue::Null)).is_err());
    assert!(
        check
            .row(&root("object_scope", SqlValue::Text("alpha-tenant".into())))
            .is_err()
    );
    assert!(
        check
            .row(&root("binary", SqlValue::Bytes(vec![0xff])))
            .is_err()
    );
    assert!(
        check
            .row(&root("binary", SqlValue::Bytes(b"ALPHA".to_vec())))
            .is_err()
    );
    check
        .row(&root(
            "scope",
            SqlValue::Json(json!([{"ordinary":["alpha-tenant"]}])),
        ))
        .unwrap();
    for value in [
        json!(["alpha-tenant"]),
        json!({"alpha-tenant":"safe"}),
        json!([{"scope_id":"alpha-tenant"}]),
    ] {
        assert!(check.row(&root("scope", SqlValue::Json(value))).is_err());
    }
}

#[test]
fn longer_logical_tenant_is_replaced_before_shorter_marker_shaped_tenant() {
    let logical = BTreeSet::from(["logical-tenant".into(), "zebra-long-tenant".into()]);
    let check = Check::new(&["<<logical-tenant>>".into()], &logical).unwrap();
    // 只有先替换长租户，后续短租户规则才会匹配刚生成的标识内部。
    let row = Row::from([("body".into(), SqlValue::Text("zebra-long-tenant".into()))]);
    assert!(check.row(&row).is_err());
}
