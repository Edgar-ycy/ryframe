use super::*;

#[test]
fn digests_require_exact_lowercase_hex_json_strings() {
    let base = config(1, 1, false, &[1]);
    for field in ["roles_sha256", "permissions_sha256"] {
        let pointer = format!("/contract/identity_pools/readers/{field}");
        for digest in [json!("c".repeat(64)), json!("0123456789abcdef".repeat(4))] {
            let config = changed(&base, &pointer, digest.clone());
            let selected = selection::workload(&config, "api", "list", 1)
                .expect("摘要为 64 位小写十六进制字符串");
            assert_eq!(selected["evidence"][field], digest);
            let receipt = selection::validate_suite(&config, "api", 1).expect("合法摘要收据");
            assert_eq!(receipt["selections"][0][field], digest);
        }
        for digest in [
            Value::Null,
            json!(true),
            json!(false),
            json!(123),
            json!(1.0),
            json!({}),
            json!([]),
            json!(["c".repeat(64)]),
            json!([["c".repeat(64)]]),
            json!([[["c".repeat(64)]]]),
            json!(["c".repeat(64), null]),
            json!(""),
            json!("A".repeat(64)),
            json!("g".repeat(64)),
            json!("c".repeat(63)),
            json!("c".repeat(65)),
            json!(format!("{}\n", "c".repeat(64))),
        ] {
            let invalid = changed(&base, &pointer, digest);
            assert_eq!(selection::workload(&invalid, "api", "list", 1), Err(POOL));
            assert_eq!(selection::validate_suite(&invalid, "api", 1), Err(POOL));
        }
    }
}

#[test]
fn constructor_tenant_has_normal_distribution_and_redacted_receipt() {
    let mut config = config(4, 2, false, &[2, 4]);
    for index in [0, 2] {
        config["contract"]["identity_pools"]["readers"]["slots"][index]["tenant_slot"] =
            json!("constructor");
        config["bindings"]["identity_pools"]["readers"][index]["tenant_id"] =
            json!("actual-constructor");
    }
    for count in [2, 4] {
        let distribution = json!({"constructor": count / 2, "tenant-2": count / 2});
        config["contract"]["identity_pools"]["readers"]["selections"][count.to_string()] =
            distribution;
    }
    let original = config.clone();
    for count in [2, 4] {
        let selected = selection::workload(&config, "api", "list", count)
            .expect("constructor 是合法普通逻辑租户名");
        assert_eq!(
            selected["evidence"]["tenant_distribution"],
            json!({"constructor": count / 2, "tenant-2": count / 2})
        );
        assert_eq!(
            selected["evidence"]["slots"][0]["tenant_slot"],
            "constructor"
        );
        assert_eq!(selected["identities"][0]["tenant_id"], "actual-constructor");
        let receipt = selection::validate_suite(&config, "api", count).expect("合法分布收据");
        assert_eq!(
            receipt,
            json!({
                "format_version": 1, "suite": "api", "selections": [selected["evidence"]],
            })
        );
        let encoded = receipt.to_string();
        assert!(encoded.contains("constructor"));
        for forbidden in [
            "actual-constructor",
            "actual-tenant",
            "username",
            "password",
        ] {
            assert!(!encoded.contains(forbidden));
        }
    }
    assert_eq!(config, original);
    let invalid = changed(
        &config,
        "/contract/identity_pools/readers/selections/4/constructor",
        json!(1),
    );
    assert_eq!(
        selection::workload(&invalid, "api", "list", 4),
        Err(DISTRIBUTION)
    );
}

#[test]
fn workflow_lookup_requires_one_match_and_suite_receipts_follow_input_order() {
    let base = config(1, 1, false, &[1]);
    assert_eq!(
        selection::workload(&base, "missing", "list", 1),
        Err(WORKFLOW)
    );
    assert_eq!(
        selection::workload(&base, "api", "missing", 1),
        Err(WORKFLOW)
    );
    for rows in [json!([]), json!({}), Value::Null] {
        let config = changed(&base, "/contract/workloads/api", rows);
        assert_eq!(
            selection::validate_suite(&config, "api", 1),
            Err("当前场景没有固定身份池")
        );
    }
    let mut duplicate = base.clone();
    duplicate["contract"]["workloads"]["api"] = json!([
        {"name": "list", "identity_pool": "readers"},
        {"name": "list", "identity_pool": "readers"},
    ]);
    assert_eq!(
        selection::workload(&duplicate, "api", "list", 1),
        Err(WORKFLOW)
    );
    let mut ordered = base;
    ordered["contract"]["workloads"]["api"] = json!([
        {"name": "z", "identity_pool": "readers"},
        {"name": "a", "identity_pool": "readers"},
    ]);
    let receipt = selection::validate_suite(&ordered, "api", 1).expect("声明顺序");
    assert_eq!(receipt["selections"][0]["workflow"], "z");
    assert_eq!(receipt["selections"][1]["workflow"], "a");
}

#[test]
fn workflow_names_keep_js_strict_equality_and_missing_property_semantics() {
    let base = config(1, 1, false, &[1]);
    for name in [
        json!(1),
        json!(true),
        Value::Null,
        json!(["list"]),
        json!({"x": 1}),
    ] {
        let mut config = base.clone();
        config["contract"]["workloads"]["api"][0]["name"] = name.clone();
        assert_eq!(
            selection::workload(&config, "api", "list", 1),
            Err(WORKFLOW)
        );
        let receipt = selection::validate_suite(&config, "api", 1).expect("保留原始 JSON 名称");
        assert_eq!(receipt["selections"][0]["workflow"], name);
    }
    let mut missing = base.clone();
    missing["contract"]["workloads"]["api"][0]
        .as_object_mut()
        .expect("场景")
        .remove("name");
    let receipt = selection::validate_suite(&missing, "api", 1).expect("缺失属性不会变成 null");
    assert!(receipt["selections"][0].get("workflow").is_none());
    let mut numbers = base;
    numbers["contract"]["workloads"]["api"] = json!([
        {"name": 1, "identity_pool": "readers"},
        {"name": 1.0, "identity_pool": "readers"},
    ]);
    assert_eq!(selection::validate_suite(&numbers, "api", 1), Err(WORKFLOW));
}

#[test]
fn selected_count_accepts_both_inclusive_bounds_without_filling_or_reducing() {
    for count in [1, 10_000] {
        let config = config(count, 1, false, &[count]);
        let selected =
            selection::workload(&config, "api", "list", count as u64).expect("身份数量边界有效");
        assert_eq!(
            selected["identities"].as_array().expect("身份").len(),
            count
        );
        assert_eq!(selected["evidence"]["count"], count);
        assert_eq!(
            selected["evidence"]["slots"]
                .as_array()
                .expect("槽位")
                .len(),
            count
        );
    }
}

#[test]
fn pool_names_preserve_ascii_pattern_and_sixty_four_character_boundary() {
    let base = config(1, 1, false, &[1]);
    for name in [
        json!(""),
        json!("A"),
        json!("0a"),
        json!("a_"),
        json!("a\n"),
        json!("a".repeat(65)),
        json!(true),
        json!(["readers"]),
        Value::Null,
    ] {
        let invalid = changed(&base, "/contract/workloads/api/0/identity_pool", name);
        assert_eq!(selection::workload(&invalid, "api", "list", 1), Err(POOL));
    }
    let name = "a".repeat(64);
    let mut renamed = base;
    for pointer in ["/contract/identity_pools", "/bindings/identity_pools"] {
        let pools = renamed
            .pointer_mut(pointer)
            .expect("池集合")
            .as_object_mut()
            .expect("池对象");
        let pool = pools.remove("readers").expect("原池");
        pools.insert(name.clone(), pool);
    }
    renamed["contract"]["workloads"]["api"][0]["identity_pool"] = json!(name);
    let selected = selection::workload(&renamed, "api", "list", 1).expect("最长合法池名称");
    assert_eq!(selected["evidence"]["identity_pool"], name);
}

#[test]
fn system_is_excluded_from_ten_tenant_workloads_even_with_matching_declarations() {
    let mut base = config(10, 10, false, &[10]);
    let pool = &mut base["contract"]["identity_pools"]["readers"];
    pool["slots"][0]["tenant_slot"] = json!("system");
    let declared = pool["selections"]["10"].as_object_mut().expect("固定分布");
    declared.remove("tenant-1");
    declared.insert("system".into(), json!(1));
    base["bindings"]["identity_pools"]["readers"][0]["tenant_id"] = json!("system");
    assert!(selection::workload(&base, "api", "list", 10).is_ok());
    for (suite, name) in [("tenants", "export"), ("jobs", "import")] {
        let config = scenario(base.clone(), suite, name);
        assert_eq!(
            selection::workload(&config, suite, name, 10),
            Err(TEN_TENANTS)
        );
    }
}

#[test]
fn validation_order_preserves_original_fixed_error_categories() {
    let mut base = config(1, 1, false, &[1]);
    base["bindings"]["identities"] = Value::Null;
    assert_eq!(selection::workload(&base, "api", "list", 0), Err(COUNT));
    assert_eq!(
        selection::workload(&base, "missing", "list", 0),
        Err(WORKFLOW)
    );
    assert_eq!(
        selection::validate_suite(&base, "missing", 0),
        Err("当前场景没有固定身份池")
    );
    base["contract"]["workloads"]["api"][0]["identity_pool"] = json!("missing");
    assert_eq!(
        selection::workload(&base, "api", "list", 1),
        Err("旧 identities 结构不可使用，必须声明独立身份池")
    );
    let mut home = home_config(1);
    home["bindings"]["identities"] = json!([]);
    home["contract"]["cycles"] = json!(true);
    assert_eq!(selection::validate_suite(&home, "homepage", 1), Err(COUNT));
}
