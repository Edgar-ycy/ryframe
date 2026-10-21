use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::devex::runtime_pacing as model;

use model::{matched_rule, validate_authority, validate_contract};

const CONTRACT: &str = "固定节奏必须声明真实限流摘要、请求/接口间隔与样本准备窗口";
const SHAPE: &str = "限流 authority 结构不完整";
const LIMIT: &str = "限流 authority 的容量或窗口无效";
const API: &str = "接口限流 authority 无效";
const REQUEST: &str = "固定请求节奏或样本准备窗口不足以满足实际全局/用户限流";
const OPERATION: &str = "节奏声明包含契约中不存在的 operation";
const RULE: &str = "限流规则无法用当前契约明确关联操作";
const GAP: &str = "固定节奏不满足实际接口或登录限流";

fn catalog() -> BTreeMap<String, Value> {
    [
        ("get_auth_csrf", "GET", "/api/v1/auth/csrf"),
        ("post_auth_login", "POST", "/api/v1/auth/login"),
        ("post_auth_refresh", "POST", "/api/v1/auth/refresh"),
        ("post_auth_logout", "POST", "/api/v1/auth/logout"),
        (
            "post_auth_password_reset_complete",
            "POST",
            "/api/v1/auth/password-reset/complete",
        ),
        ("get_system_posts", "GET", "/api/v1/system/posts"),
    ]
    .into_iter()
    .map(|(name, method, path)| (name.to_owned(), json!({"method": method, "path": path})))
    .collect()
}

fn authority() -> Value {
    json!({
        "format_version": 1,
        "global": {"capacity": 100, "window_ms": 60000},
        "user": {"enabled": false, "capacity": 500, "window_ms": 60000},
        "api": {"window_ms": 60000, "rules": {
            "POST /api/v1/auth/login": 5,
            "GET /api/v1/auth/csrf": 30,
            "POST /api/v1/auth/password-reset/complete": 3
        }},
        "login": {"capacity": 5, "window_ms": 60000}
    })
}

fn pacing() -> Value {
    json!({
        "authority_sha256": "a".repeat(64),
        "request_interval_ms": 1000,
        "sample_prepare_wait_ms": 61250,
        "operation_intervals_ms": {
            "get_auth_csrf": 2250,
            "post_auth_login": 12250,
            "post_auth_password_reset_complete": 20250
        }
    })
}

fn at(value: &mut Value, pointer: &str, replacement: Value) {
    *value.pointer_mut(pointer).expect("测试字段必须存在") = replacement;
}

fn check(pacing: &Value, authority: &Value) -> Result<(), &'static str> {
    validate_authority(pacing, authority, &catalog())
}

#[test]
fn original_pacing_fixture_and_all_six_original_mutations_are_preserved() {
    assert_eq!(check(&pacing(), &authority()), Ok(()));
    for (pointer, replacement, error) in [
        ("/request_interval_ms", json!(600), REQUEST),
        ("/sample_prepare_wait_ms", json!(60000), REQUEST),
        ("/operation_intervals_ms/get_auth_csrf", json!(2000), GAP),
        ("/operation_intervals_ms/post_auth_login", json!(12000), GAP),
    ] {
        let mut value = pacing();
        at(&mut value, pointer, replacement);
        assert_eq!(check(&value, &authority()), Err(error));
    }
    let mut value = pacing();
    value["operation_intervals_ms"]
        .as_object_mut()
        .unwrap()
        .remove("post_auth_password_reset_complete");
    assert_eq!(check(&value, &authority()), Err(GAP));
    value = pacing();
    value["operation_intervals_ms"]["unknown"] = json!(1000);
    assert_eq!(check(&value, &authority()), Err(OPERATION));
}

#[test]
fn original_enabled_user_limit_and_unmatched_rule_cases_are_preserved() {
    let mut limits = authority();
    limits["user"] = json!({"enabled": true, "capacity": 10, "window_ms": 60000});
    assert_eq!(check(&pacing(), &limits), Err(REQUEST));
    let mut value = pacing();
    value["request_interval_ms"] = json!(6500);
    assert_eq!(check(&value, &limits), Ok(()));
    limits["api"]["rules"]["/unmatched"] = json!(1);
    assert_eq!(check(&value, &limits), Err(RULE));
}

#[test]
fn exact_spacing_and_prepare_margin_boundaries_are_inclusive() {
    let mut value = pacing();
    value["request_interval_ms"] = json!(601);
    value["sample_prepare_wait_ms"] = json!(60250);
    value["operation_intervals_ms"] = json!({
        "get_auth_csrf": 2001,
        "post_auth_login": 12001,
        "post_auth_password_reset_complete": 20001
    });
    assert_eq!(check(&value, &authority()), Ok(()));
    for (pointer, replacement, error) in [
        ("/request_interval_ms", json!(600), REQUEST),
        ("/sample_prepare_wait_ms", json!(60249), REQUEST),
        ("/operation_intervals_ms/get_auth_csrf", json!(2000), GAP),
        ("/operation_intervals_ms/post_auth_login", json!(12000), GAP),
        (
            "/operation_intervals_ms/post_auth_password_reset_complete",
            json!(20000),
            GAP,
        ),
    ] {
        let mut changed = value.clone();
        at(&mut changed, pointer, replacement);
        assert_eq!(check(&changed, &authority()), Err(error));
    }
}

#[test]
fn fast_authority_has_no_hardcoded_request_or_prepare_minimum() {
    let mut limits = authority();
    for name in ["global", "api", "login"] {
        limits[name]["window_ms"] = json!(10);
    }
    let mut value = pacing();
    value["request_interval_ms"] = json!(15);
    value["sample_prepare_wait_ms"] = json!(300);
    value["operation_intervals_ms"] = json!({});
    assert_eq!(check(&value, &limits), Ok(()));
    limits["global"] = json!({"window_ms": 1, "capacity": 1});
    value["request_interval_ms"] = json!(1);
    assert_eq!(check(&value, &limits), Err(REQUEST));
    value["request_interval_ms"] = json!(4);
    assert_eq!(check(&value, &limits), Ok(()));
}

#[test]
fn disabled_user_is_validated_but_not_used_for_spacing_or_largest_window() {
    let mut limits = authority();
    limits["user"] = json!({"enabled": false, "capacity": 1, "window_ms": 120000});
    assert_eq!(check(&pacing(), &limits), Ok(()));
    limits["user"]["enabled"] = json!(true);
    assert_eq!(check(&pacing(), &limits), Err(REQUEST));
    let mut value = pacing();
    value["request_interval_ms"] = json!(120001);
    value["sample_prepare_wait_ms"] = json!(120250);
    assert_eq!(check(&value, &limits), Ok(()));
    limits["user"]["enabled"] = json!(false);
    limits["user"]["capacity"] = json!(0);
    assert_eq!(check(&value, &limits), Err(LIMIT));
}

#[test]
fn preparation_uses_largest_global_api_login_or_enabled_user_window() {
    for name in ["global", "api", "login", "user"] {
        let mut limits = authority();
        limits[name]["window_ms"] = json!(90000);
        limits["user"]["enabled"] = json!(true);
        let mut value = pacing();
        value["request_interval_ms"] = json!(100001);
        value["sample_prepare_wait_ms"] = json!(90250);
        assert_eq!(check(&value, &limits), Ok(()));
        value["sample_prepare_wait_ms"] = json!(90249);
        assert_eq!(check(&value, &limits), Err(REQUEST));
    }
}

#[test]
fn request_interval_uses_maximum_of_global_and_enabled_user_spacing() {
    let mut limits = authority();
    limits["user"] = json!({"enabled": true, "capacity": 10, "window_ms": 60000});
    let mut value = pacing();
    value["request_interval_ms"] = json!(6001);
    assert_eq!(check(&value, &limits), Ok(()));
    value["request_interval_ms"] = json!(6000);
    assert_eq!(check(&value, &limits), Err(REQUEST));
    limits["user"]["capacity"] = json!(1000);
    value["request_interval_ms"] = json!(601);
    assert_eq!(check(&value, &limits), Ok(()));
    value["request_interval_ms"] = json!(600);
    assert_eq!(check(&value, &limits), Err(REQUEST));
}

#[test]
fn contract_requires_exact_top_level_keys_and_object_intervals() {
    for value in [Value::Null, json!([]), json!(true), json!("pacing")] {
        assert_eq!(validate_contract(&value), Err(CONTRACT));
    }
    for field in [
        "authority_sha256",
        "operation_intervals_ms",
        "request_interval_ms",
        "sample_prepare_wait_ms",
    ] {
        let mut value = pacing();
        value.as_object_mut().unwrap().remove(field);
        assert_eq!(validate_contract(&value), Err(CONTRACT));
    }
    let mut value = pacing();
    value["extra"] = json!(true);
    assert_eq!(validate_contract(&value), Err(CONTRACT));
    for intervals in [Value::Null, json!([]), json!("operations"), json!(false)] {
        value = pacing();
        value["operation_intervals_ms"] = intervals;
        assert_eq!(validate_contract(&value), Err(CONTRACT));
    }
    value = pacing();
    value["operation_intervals_ms"] = json!({});
    assert_eq!(validate_contract(&value), Ok(()));
}

#[test]
fn digest_requires_json_string_with_exactly_64_lowercase_hex_digits() {
    for digest in [
        json!("A".repeat(64)),
        json!("a".repeat(63)),
        json!("a".repeat(65)),
        json!(format!("{}\n", "a".repeat(64))),
        json!("g".repeat(64)),
        Value::Null,
        json!(64),
        json!(true),
        json!([]),
        json!(["a", "b"]),
        json!({}),
        json!(["a".repeat(64)]),
        json!([["a".repeat(64)]]),
    ] {
        let mut value = pacing();
        value["authority_sha256"] = digest;
        assert_eq!(validate_contract(&value), Err(CONTRACT));
    }
    for digest in [
        json!("0123456789abcdef".repeat(4)),
        json!("a".repeat(64)),
        json!("0".repeat(64)),
    ] {
        let mut value = pacing();
        value["authority_sha256"] = digest;
        assert_eq!(validate_contract(&value), Ok(()));
    }
}

#[test]
fn intervals_use_js_safe_integer_types_and_positive_bounded_values() {
    for invalid in [
        Value::Null,
        json!(true),
        json!("1000"),
        json!(0),
        json!(-1),
        json!(-0.0),
        json!(1.5),
        json!(86400251),
        json!(9007199254740992_u64),
        serde_json::from_str("1e999").unwrap(),
    ] {
        for pointer in [
            "/request_interval_ms",
            "/sample_prepare_wait_ms",
            "/operation_intervals_ms/get_auth_csrf",
        ] {
            let mut value = pacing();
            at(&mut value, pointer, invalid.clone());
            assert_eq!(validate_contract(&value), Err(CONTRACT));
        }
    }
    for valid in [
        json!(1),
        json!(1.0),
        json!(86400250),
        serde_json::from_str("1e3").unwrap(),
        serde_json::from_str("1000.00000000000000001").unwrap(),
    ] {
        let mut value = pacing();
        for pointer in [
            "/request_interval_ms",
            "/sample_prepare_wait_ms",
            "/operation_intervals_ms/get_auth_csrf",
        ] {
            at(&mut value, pointer, valid.clone());
        }
        assert_eq!(validate_contract(&value), Ok(()));
    }
}

#[test]
fn operation_names_are_exact_lowercase_ascii_identifiers() {
    for name in [
        "",
        "_name",
        "1name",
        "Name",
        "with-dash",
        "a.b",
        "a/b",
        "a b",
        "中文",
        "a\n",
    ] {
        let mut value = pacing();
        value["operation_intervals_ms"] = json!({(name): 1000});
        assert_eq!(validate_contract(&value), Err(CONTRACT));
    }
    for name in ["a", "a0", "post_auth_login", "a__1"] {
        let mut value = pacing();
        value["operation_intervals_ms"] = json!({(name): 1000});
        assert_eq!(validate_contract(&value), Ok(()));
    }
}

#[test]
fn authority_shape_requires_numeric_version_boolean_enabled_and_object_rules() {
    for (pointer, replacement) in [
        ("/format_version", json!("1")),
        ("/format_version", json!(true)),
        ("/format_version", json!(2)),
        ("/user/enabled", Value::Null),
        ("/user/enabled", json!(0)),
        ("/user/enabled", json!("false")),
        ("/api/rules", json!([])),
        ("/api/rules", Value::Null),
    ] {
        let mut limits = authority();
        at(&mut limits, pointer, replacement);
        assert_eq!(check(&pacing(), &limits), Err(SHAPE));
    }
    for limits in [Value::Null, json!([]), json!({})] {
        assert_eq!(check(&pacing(), &limits), Err(SHAPE));
    }
    let mut limits = authority();
    limits["format_version"] = json!(1.0);
    limits["extra"] = json!(true);
    limits["global"]["extra"] = json!(true);
    assert_eq!(check(&pacing(), &limits), Ok(()));
}

#[test]
fn all_global_user_login_and_api_windows_are_positive_bounded_safe_integers() {
    for pointer in [
        "/global/window_ms",
        "/user/window_ms",
        "/login/window_ms",
        "/api/window_ms",
    ] {
        for invalid in [
            Value::Null,
            json!(0),
            json!(-1),
            json!(1.5),
            json!("60000"),
            json!(true),
            json!(86400251),
        ] {
            let mut limits = authority();
            at(&mut limits, pointer, invalid);
            let error = if pointer == "/api/window_ms" {
                API
            } else {
                LIMIT
            };
            assert_eq!(check(&pacing(), &limits), Err(error));
        }
    }
}

#[test]
fn all_capacities_require_positive_js_safe_integers_even_for_disabled_user() {
    for pointer in [
        "/global/capacity",
        "/user/capacity",
        "/login/capacity",
        "/api/rules/POST ~1api~1v1~1auth~1login",
    ] {
        for invalid in [
            Value::Null,
            json!(0),
            json!(-1),
            json!(1.5),
            json!("5"),
            json!(true),
            json!(9007199254740992_u64),
        ] {
            let mut limits = authority();
            at(&mut limits, pointer, invalid);
            let error = if pointer.starts_with("/api/") {
                API
            } else {
                LIMIT
            };
            assert_eq!(check(&pacing(), &limits), Err(error));
        }
    }
}

#[test]
fn missing_authority_fields_do_not_acquire_defaults() {
    for name in ["global", "user", "login"] {
        for field in ["capacity", "window_ms"] {
            let mut limits = authority();
            limits[name].as_object_mut().unwrap().remove(field);
            assert_eq!(check(&pacing(), &limits), Err(LIMIT));
        }
    }
    for (group, field, error) in [
        ("api", "window_ms", API),
        ("api", "rules", SHAPE),
        ("user", "enabled", SHAPE),
    ] {
        let mut limits = authority();
        limits[group].as_object_mut().unwrap().remove(field);
        assert_eq!(check(&pacing(), &limits), Err(error));
    }
}

#[test]
fn integer_valued_float_capacities_keep_js_number_semantics() {
    let mut limits = authority();
    for name in ["global", "user", "login"] {
        limits[name]["capacity"] = json!(1.0);
    }
    limits["api"]["rules"] = json!({"POST": 1.0, "GET": 1.0});
    let mut value = pacing();
    value["request_interval_ms"] = json!(60001);
    value["operation_intervals_ms"] = json!({});
    assert_eq!(check(&value, &limits), Ok(()));
}

#[test]
fn capacities_above_interval_bound_up_to_js_safe_maximum_remain_valid() {
    let mut limits = authority();
    for name in ["global", "user", "login"] {
        limits[name]["capacity"] = json!(9007199254740991_u64);
        limits[name]["window_ms"] = json!(1.0);
    }
    limits["api"]["window_ms"] = json!(1.0);
    limits["api"]["rules"] = json!({"POST": 9007199254740991_u64});
    limits["user"]["enabled"] = json!(true);
    let mut value = pacing();
    value["request_interval_ms"] = json!(1.0);
    value["sample_prepare_wait_ms"] = json!(251);
    value["operation_intervals_ms"] = json!({});
    assert_eq!(check(&value, &limits), Ok(()));
}

#[test]
fn maximum_window_and_added_margin_do_not_wrap_or_relax_interval_limit() {
    let mut limits = authority();
    limits["global"] = json!({"window_ms": 86400000, "capacity": 1});
    let mut value = pacing();
    value["request_interval_ms"] = json!(86400250);
    value["sample_prepare_wait_ms"] = json!(86400250);
    assert_eq!(check(&value, &limits), Ok(()));
    limits["global"]["window_ms"] = json!(86400250);
    assert_eq!(check(&value, &limits), Err(REQUEST));
}

#[test]
fn matched_rule_uses_exact_method_path_then_path_then_method_priority() {
    let operation = json!({"method": "GET", "path": "/api/v1/system/posts"});
    let mut rules = json!({"GET /api/v1/system/posts": 1, "/api/v1/system/posts": 2, "GET": 3});
    for (expected, remove) in [
        ("GET /api/v1/system/posts", "GET /api/v1/system/posts"),
        ("/api/v1/system/posts", "/api/v1/system/posts"),
        ("GET", "GET"),
    ] {
        assert_eq!(matched_rule(&operation, &rules), Some(expected.into()));
        rules.as_object_mut().unwrap().remove(remove);
    }
    assert_eq!(matched_rule(&operation, &rules), None);
}

#[test]
fn matched_rule_checks_own_key_presence_without_normalization_or_value_truthiness() {
    let operation = json!({"method": "GET", "path": "/posts"});
    assert_eq!(
        matched_rule(&operation, &json!({"GET /posts": null})),
        Some("GET /posts".into())
    );
    assert_eq!(
        matched_rule(&operation, &json!({"/posts": false, "GET": 1})),
        Some("/posts".into())
    );
    assert_eq!(
        matched_rule(
            &operation,
            &json!({"get": 1, "/posts/": 1, "/post": 1, "*": 1})
        ),
        None
    );
    assert_eq!(
        matched_rule(&json!({"method": "constructor", "path": "/x"}), &json!({})),
        None
    );
}

#[test]
fn shadowed_method_and_path_rules_are_still_associated_with_catalog() {
    let mut limits = authority();
    limits["api"]["rules"]["GET"] = json!(1);
    limits["api"]["rules"]["/api/v1/auth/csrf"] = json!(1);
    limits["api"]["rules"]["GET /api/v1/system/posts"] = json!(100);
    assert_eq!(check(&pacing(), &limits), Ok(()));
    limits["api"]["rules"]["/api/v1/*"] = json!(1);
    assert_eq!(check(&pacing(), &limits), Err(RULE));
}

#[test]
fn every_declared_operation_must_exist_but_empty_catalog_and_rules_are_allowed() {
    let mut operations = catalog();
    operations.remove("get_auth_csrf");
    assert_eq!(
        validate_authority(&pacing(), &authority(), &operations),
        Err(OPERATION)
    );
    let mut limits = authority();
    limits["api"]["rules"] = json!({});
    let mut value = pacing();
    value["operation_intervals_ms"] = json!({});
    assert_eq!(
        validate_authority(&value, &limits, &BTreeMap::new()),
        Ok(())
    );
}

#[test]
fn global_interval_can_satisfy_api_and_login_limits_and_smaller_overrides_do_not_reduce_it() {
    let mut value = pacing();
    value["request_interval_ms"] = json!(20001);
    value["operation_intervals_ms"] = json!({});
    assert_eq!(check(&value, &authority()), Ok(()));
    value["operation_intervals_ms"] = json!({"get_auth_csrf": 1, "post_auth_login": 1});
    assert_eq!(check(&value, &authority()), Ok(()));
}

#[test]
fn login_spacing_is_required_without_api_rule_and_only_for_exact_login_operation_name() {
    let mut limits = authority();
    limits["global"]["window_ms"] = json!(1);
    limits["api"] = json!({"window_ms": 1, "rules": {}});
    limits["login"] = json!({"window_ms": 1000, "capacity": 2});
    let mut value = pacing();
    value["request_interval_ms"] = json!(1);
    value["sample_prepare_wait_ms"] = json!(1250);
    value["operation_intervals_ms"] = json!({"post_auth_login": 501});
    assert_eq!(check(&value, &limits), Ok(()));
    value["operation_intervals_ms"]["post_auth_login"] = json!(500);
    assert_eq!(check(&value, &limits), Err(GAP));
    let login = catalog().remove("post_auth_login").unwrap();
    value["operation_intervals_ms"] = json!({});
    assert_eq!(
        validate_authority(
            &value,
            &limits,
            &BTreeMap::from([("post_custom".into(), login)])
        ),
        Ok(())
    );
}

#[test]
fn login_uses_stricter_api_or_login_spacing_in_both_directions() {
    for login_window in [1000, 120000] {
        let mut limits = authority();
        limits["login"]["window_ms"] = json!(login_window);
        let required = if login_window == 1000 { 12001 } else { 24001 };
        let mut value = pacing();
        value["sample_prepare_wait_ms"] = json!(120250);
        value["operation_intervals_ms"]["post_auth_login"] = json!(required);
        assert_eq!(check(&value, &limits), Ok(()));
        value["operation_intervals_ms"]["post_auth_login"] = json!(required - 1);
        assert_eq!(check(&value, &limits), Err(GAP));
    }
}

#[test]
fn every_operation_using_same_api_rule_must_have_sufficient_spacing() {
    let mut limits = authority();
    limits["api"]["rules"] = json!({"GET": 1});
    let mut value = pacing();
    value["operation_intervals_ms"]["get_auth_csrf"] = json!(60001);
    assert_eq!(check(&value, &limits), Err(GAP));
    value["operation_intervals_ms"]["get_system_posts"] = json!(60001);
    assert_eq!(check(&value, &limits), Ok(()));
}

#[test]
fn validation_order_preserves_contract_shape_limits_spacing_operations_and_rules() {
    assert_eq!(
        validate_authority(&Value::Null, &Value::Null, &catalog()),
        Err(CONTRACT)
    );
    let mut limits = authority();
    limits["global"]["capacity"] = json!(0);
    limits["api"]["window_ms"] = json!(0);
    assert_eq!(check(&pacing(), &limits), Err(LIMIT));
    let mut value = pacing();
    value["request_interval_ms"] = json!(1);
    value["operation_intervals_ms"]["unknown"] = json!(1000);
    assert_eq!(check(&value, &authority()), Err(REQUEST));
    value["request_interval_ms"] = json!(1000);
    limits = authority();
    limits["api"]["rules"]["/unknown"] = json!(1);
    assert_eq!(check(&value, &limits), Err(OPERATION));
    value = pacing();
    value["operation_intervals_ms"] = json!({});
    assert_eq!(check(&value, &limits), Err(RULE));
}

#[test]
fn authority_validation_does_not_add_digest_comparison_or_mutate_inputs() {
    let mut value = pacing();
    value["authority_sha256"] = json!("0".repeat(64));
    let limits = authority();
    let operations = catalog();
    let original = (value.clone(), limits.clone(), operations.clone());
    assert_eq!(validate_authority(&value, &limits, &operations), Ok(()));
    assert_eq!((value, limits, operations), original);
}
