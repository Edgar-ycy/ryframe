//! 固定身份池选择、分布校验和脱敏收据的纯模型测试。

use crate::devex::runtime_selection as selection;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const COUNT: &str = "身份池选中数量无效";
const POOL: &str = "身份池必须固定角色权限摘要、逻辑槽位及客户地址模型";
const BINDING: &str = "身份池槽位、固定地址或具体身份绑定无效";
const DISTINCT: &str = "实际选中身份必须为独立用户、逻辑槽位与固定地址";
const TENANT: &str = "逻辑租户与实际租户必须保持一一对应";
const DISTRIBUTION: &str = "实际前 N 身份的租户分布必须匹配固定负载声明";
const WORKFLOW: &str = "必须唯一声明当前身份池所属场景";
const TEN_TENANTS: &str = "实际并发用户必须均匀覆盖十个普通租户";

#[path = "runtime_selection/fixture.rs"]
mod fixture;
use fixture::{changed, config, home_config, jobs_config, scenario};

#[test]
fn prefix_selection_is_exact() {
    let mut config = config(3, 1, false, &[2]);
    let result = selection::workload(&config, "api", "list", 2).expect("选择前两项");
    assert_eq!(result["identities"].as_array().expect("身份数组").len(), 2);
    config["bindings"]["identity_pools"]["readers"][2] = Value::Null;
    config["contract"]["identity_pools"]["readers"]["slots"][2] = Value::Null;
    assert_eq!(selection::workload(&config, "api", "list", 2), Ok(result));
    config["bindings"]["identity_pools"]["readers"][1] =
        config["bindings"]["identity_pools"]["readers"][0].clone();
    assert_eq!(
        selection::workload(&config, "api", "list", 2),
        Err(DISTINCT)
    );
    config["bindings"]["identity_pools"]["readers"]
        .as_array_mut()
        .expect("绑定")
        .truncate(1);
    assert_eq!(
        selection::workload(&config, "api", "list", 2),
        Err("身份池缺少实际选中的独立用户")
    );
    for count in [0, 10_001, 9_007_199_254_740_992, u64::MAX] {
        assert_eq!(
            selection::workload(&config, "api", "list", count),
            Err(COUNT)
        );
    }
}

#[test]
fn ten_tenant_prefixes_match_all_declared_concurrencies() {
    for (suite, workflow) in [("tenants", "export"), ("jobs", "import")] {
        let selected_config = scenario(config(100, 10, false, &[10, 50, 100]), suite, workflow);
        for count in [10, 50, 100] {
            let selected =
                selection::workload(&selected_config, suite, workflow, count).expect("十租户负载");
            assert_eq!(
                selected["identities"].as_array().expect("身份").len(),
                count as usize
            );
            let receipt =
                selection::validate_suite(&selected_config, suite, count).expect("分布收据");
            let distribution = receipt["selections"][0]["tenant_distribution"]
                .as_object()
                .expect("分布");
            assert_eq!(distribution.len(), 10);
            assert!(distribution.values().all(|amount| amount == (count / 10)));
        }
        let wrong = changed(
            &selected_config,
            "/bindings/identity_pools/readers/9/tenant_id",
            json!("actual-tenant-1"),
        );
        assert_eq!(selection::validate_suite(&wrong, suite, 10), Err(TENANT));
        let nine = scenario(config(100, 9, false, &[10]), suite, workflow);
        assert_eq!(
            selection::validate_suite(&nine, suite, 10),
            Err(TEN_TENANTS)
        );
    }
}

#[test]
fn jobs_select_their_own_pools() {
    let mut config = jobs_config(10);
    let imports = selection::workload(&config, "jobs", "import", 10).expect("导入池");
    let schedule = selection::workload(&config, "jobs", "schedule", 10).expect("系统池");
    assert_eq!(imports["evidence"]["identity_pool"], "imports");
    assert_eq!(schedule["evidence"]["identity_pool"], "system");
    assert!(
        imports["identities"]
            .as_array()
            .expect("导入身份")
            .iter()
            .all(|row| row["tenant_id"] != "system")
    );
    assert!(
        schedule["identities"]
            .as_array()
            .expect("调度身份")
            .iter()
            .all(|row| row["tenant_id"] == "system")
    );
    let receipt = selection::validate_suite(&config, "jobs", 10).expect("场景收据");
    assert_eq!(
        receipt["selections"],
        json!([imports["evidence"], schedule["evidence"]])
    );
    config["contract"]["workloads"]["jobs"][1]["identity_pool"] = json!("imports");
    assert_eq!(
        selection::validate_suite(&config, "jobs", 10),
        Err("当前调度目标只能使用 system 普通身份池")
    );
}

#[test]
fn original_invalid_pool_cases_fail_closed() {
    let base = config(10, 1, false, &[10]);
    let cases = [
        (
            "/contract/identity_pools/readers/permissions_sha256",
            json!("invalid"),
            POOL,
        ),
        (
            "/contract/identity_pools/readers/client_address_model",
            json!("rotating"),
            POOL,
        ),
        (
            "/contract/identity_pools/readers/slots/1/client_address",
            json!("198.18.10.1"),
            DISTINCT,
        ),
        (
            "/contract/identity_pools/readers/slots/0/client_address",
            json!("203.0.113.1"),
            BINDING,
        ),
        (
            "/contract/identity_pools/readers/slots/1/user_slot",
            json!("user-1"),
            DISTINCT,
        ),
        (
            "/contract/identity_pools/readers/selections/10",
            json!({"tenant-1": 9}),
            DISTRIBUTION,
        ),
        (
            "/bindings/identity_pools/readers/1/tenant_id",
            json!("system"),
            TENANT,
        ),
        (
            "/bindings/identity_pools/readers/1/password_env",
            json!("secret-value"),
            BINDING,
        ),
        ("/bindings/identity_pools/readers/1", json!({}), BINDING),
        (
            "/contract/workloads/api/0/identity_pool",
            json!("missing"),
            POOL,
        ),
    ];
    for (pointer, value, reason) in cases {
        let invalid = changed(&base, pointer, value);
        assert_eq!(selection::validate_suite(&invalid, "api", 10), Err(reason));
    }
    let mut legacy = base.clone();
    legacy["bindings"]["identities"] = json!([]);
    assert_eq!(
        selection::validate_suite(&legacy, "api", 10),
        Err("旧 identities 结构不可使用，必须声明独立身份池")
    );
    let mut missing = base.clone();
    missing["contract"]["identity_pools"]["readers"]
        .as_object_mut()
        .expect("池")
        .remove("roles_sha256");
    assert_eq!(selection::validate_suite(&missing, "api", 10), Err(POOL));
    let mut extra = base;
    extra["bindings"]["identity_pools"]["readers"][1]["token"] = json!("forbidden");
    assert_eq!(selection::validate_suite(&extra, "api", 10), Err(BINDING));
}

#[test]
fn contract_hash_and_redacted_receipt() {
    let base = config(10, 1, false, &[10]);
    let hash = |value: &Value| Sha256::digest(serde_json::to_vec(value).expect("序列化契约"));
    let original = hash(&base["contract"]);
    for (pointer, value) in [
        (
            "/contract/identity_pools/readers/roles_sha256",
            json!("c".repeat(64)),
        ),
        (
            "/contract/identity_pools/readers/permissions_sha256",
            json!("d".repeat(64)),
        ),
        (
            "/contract/identity_pools/readers/selections/10/tenant-1",
            json!(9),
        ),
        (
            "/contract/identity_pools/readers/slots/0/client_address",
            json!("198.18.20.1"),
        ),
    ] {
        assert_ne!(hash(&changed(&base, pointer, value)["contract"]), original);
    }
    let receipt = selection::validate_suite(&base, "api", 10).expect("脱敏收据");
    let encoded = receipt.to_string();
    for forbidden in [
        "actual-tenant",
        "password_env",
        "RYFRAME_SELECTION",
        "username",
        "tenant_id",
    ] {
        assert!(!encoded.contains(forbidden));
    }
    assert!(encoded.contains("roles_sha256") && encoded.contains("client_address"));
    assert_eq!(receipt["format_version"], 1);
    assert_eq!(receipt["suite"], "api");
    assert_eq!(receipt["selections"][0]["workflow"], "list");
    assert_eq!(receipt["selections"][0]["count"], 10);
}

#[test]
fn selected_cardinality_never_reduces_admission() {
    for count in [10, 50, 100] {
        let config = jobs_config(count);
        for workflow in ["import", "schedule"] {
            let selected = selection::workload(&config, "jobs", workflow, count as u64)
                .expect("保留全部并发身份");
            let identities = selected["identities"].as_array().expect("身份数组");
            assert_eq!(identities.len(), count);
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
}

#[test]
fn homepage_preserves_order_and_fixed_addresses() {
    let config = home_config(10);
    let original = config.clone();
    let selected = selection::homepage(&config, 10).expect("首页身份");
    for (index, identity) in selected["identities"]
        .as_array()
        .expect("身份数组")
        .iter()
        .enumerate()
    {
        let mut expected = config["bindings"]["identity_pools"]["readers"][index].clone();
        let slot = &config["contract"]["identity_pools"]["readers"]["slots"][index];
        expected["client_address"] = slot["client_address"].clone();
        assert_eq!(*identity, expected);
    }
    assert_eq!(selection::homepage(&config, 10), Ok(selected.clone()));
    assert_eq!(config, original);
    assert!(selected["evidence"].get("workflow").is_none());
    let mut evidence = selected["evidence"].clone();
    evidence["workflow"] = json!("homepage");
    assert_eq!(
        selection::validate_suite(&config, "homepage", u64::MAX),
        Ok(json!({"format_version": 1, "suite": "homepage", "selections": [evidence]}))
    );
}

#[test]
fn homepage_rejects_invalid_selected_bindings() {
    let base = home_config(10);
    let mut missing = base.clone();
    missing["bindings"]["identity_pools"]["readers"]
        .as_array_mut()
        .expect("绑定")
        .pop();
    assert_eq!(
        selection::homepage(&missing, 10),
        Err("身份池缺少实际选中的独立用户")
    );
    for (pointer, value, reason) in [
        (
            "/bindings/identity_pools/readers/1/username",
            json!("user-1"),
            DISTINCT,
        ),
        (
            "/contract/identity_pools/readers/slots/1/client_address",
            json!("198.18.10.1"),
            DISTINCT,
        ),
        (
            "/contract/identity_pools/readers/slots/1/client_address",
            json!("203.0.113.1"),
            BINDING,
        ),
        (
            "/contract/identity_pools/readers/slots/1/client_address",
            json!("198.18.999.1"),
            BINDING,
        ),
        ("/contract/homepage", Value::Null, POOL),
    ] {
        assert_eq!(
            selection::homepage(&changed(&base, pointer, value), 10),
            Err(reason)
        );
    }
}

#[test]
fn json_integer_semantics_match_js_safe_integer_not_rust_storage_kind() {
    let mut config = home_config(2);
    config["contract"]["cycles"] = json!(2.0);
    config["contract"]["identity_pools"]["readers"]["selections"]["2"]["tenant-1"] = json!(2.0);
    assert!(selection::validate_suite(&config, "homepage", 0).is_ok());
    for value in [
        json!(true),
        json!("2"),
        Value::Null,
        json!(2.5),
        json!(-1),
        json!(0),
        json!(10_001),
        json!(9_007_199_254_740_991_u64),
        json!(9_007_199_254_740_992_u64),
        json!(1e100),
    ] {
        let wrong = changed(&config, "/contract/cycles", value);
        assert_eq!(selection::validate_suite(&wrong, "homepage", 2), Err(COUNT));
    }
    for value in [
        json!(true),
        json!("2"),
        Value::Null,
        json!(2.5),
        json!(-2),
        json!(0),
        json!(9_007_199_254_740_992_u64),
    ] {
        let wrong = changed(
            &config,
            "/contract/identity_pools/readers/selections/2/tenant-1",
            value,
        );
        assert_eq!(
            selection::validate_suite(&wrong, "homepage", 2),
            Err(DISTRIBUTION)
        );
    }
}

#[test]
fn username_length_is_utf16_and_output_keeps_original_text() {
    let base = config(1, 1, false, &[1]);
    for username in [
        "😀".repeat(32),
        "汉".repeat(64),
        "a".repeat(64),
        "  User  ".into(),
        "\u{0085}".into(),
        "\u{200b}".into(),
    ] {
        let config = changed(
            &base,
            "/bindings/identity_pools/readers/0/username",
            json!(username),
        );
        let selected = selection::workload(&config, "api", "list", 1).expect("合法 UTF-16 长度");
        assert_eq!(selected["identities"][0]["username"], username);
    }
    for username in [
        "😀".repeat(33),
        "汉".repeat(65),
        "a".repeat(65),
        format!("{}a", " ".repeat(64)),
        "".into(),
        "\u{feff}".into(),
        " \u{a0}\u{2028}\u{3000}\n".into(),
    ] {
        let config = changed(
            &base,
            "/bindings/identity_pools/readers/0/username",
            json!(username),
        );
        assert_eq!(selection::workload(&config, "api", "list", 1), Err(BINDING));
    }
}

#[test]
fn duplicate_subjects_use_js_trim_and_unicode_lowercase_without_rewriting_identity() {
    let base = config(2, 1, false, &[2]);
    for (first, second) in [
        (" Alice ", "alice"),
        ("\u{feff}USER\u{a0}", " user "),
        ("İ", "i\u{307}"),
        ("ΟΣ", "ος"),
    ] {
        let first = changed(
            &base,
            "/bindings/identity_pools/readers/0/username",
            json!(first),
        );
        let config = changed(
            &first,
            "/bindings/identity_pools/readers/1/username",
            json!(second),
        );
        assert_eq!(
            selection::workload(&config, "api", "list", 2),
            Err(DISTINCT)
        );
    }
    let different = changed(
        &base,
        "/bindings/identity_pools/readers/0/username",
        json!("\u{85}user-2"),
    );
    assert!(selection::workload(&different, "api", "list", 2).is_ok());
    let mut cross_tenant = config(2, 2, false, &[2]);
    cross_tenant["bindings"]["identity_pools"]["readers"][1]["username"] = json!("user-1");
    assert!(selection::workload(&cross_tenant, "api", "list", 2).is_ok());
}

#[test]
fn logical_and_actual_tenants_are_bijective_in_both_directions() {
    let base = config(2, 1, false, &[2]);
    let changed_tenant = changed(
        &base,
        "/bindings/identity_pools/readers/1/tenant_id",
        json!("other"),
    );
    assert_eq!(
        selection::workload(&changed_tenant, "api", "list", 2),
        Err(TENANT)
    );
    let two_slots = changed(
        &base,
        "/contract/identity_pools/readers/slots/1/tenant_slot",
        json!("tenant-2"),
    );
    assert_eq!(
        selection::workload(&two_slots, "api", "list", 2),
        Err(TENANT)
    );
    let system_slot = changed(
        &base,
        "/contract/identity_pools/readers/slots/0/tenant_slot",
        json!("system"),
    );
    assert_eq!(
        selection::workload(&system_slot, "api", "list", 2),
        Err(TENANT)
    );
    let system_identity = changed(
        &base,
        "/bindings/identity_pools/readers/0/tenant_id",
        json!("system"),
    );
    assert_eq!(
        selection::workload(&system_identity, "api", "list", 2),
        Err(TENANT)
    );
}

#[test]
fn addresses_must_be_canonical_benchmark_ipv4_and_unique() {
    let base = config(1, 1, false, &[1]);
    for address in ["198.18.0.0", "198.19.255.255", "198.18.9.10"] {
        let config = changed(
            &base,
            "/contract/identity_pools/readers/slots/0/client_address",
            json!(address),
        );
        assert!(selection::workload(&config, "api", "list", 1).is_ok());
    }
    for address in [
        "198.018.1.1",
        "198.18.01.1",
        "198.18.1.001",
        "198.18.256.1",
        "198.20.1.1",
        "198.18.-1.1",
        "198.18.+1.1",
        "198.18.1.1\n",
        "198.18.1",
        "198.18.1.1.1",
        "::1",
        "198.18.１.1",
    ] {
        let config = changed(
            &base,
            "/contract/identity_pools/readers/slots/0/client_address",
            json!(address),
        );
        assert_eq!(selection::workload(&config, "api", "list", 1), Err(BINDING));
    }
}

#[test]
fn pool_slot_and_binding_keys_are_exact_and_types_do_not_coerce() {
    let base = config(1, 1, false, &[1]);
    for (pointer, reason) in [
        ("/contract/identity_pools/readers", POOL),
        ("/contract/identity_pools/readers/slots", POOL),
        ("/contract/identity_pools/readers/selections", POOL),
        ("/bindings/identity_pools/readers", POOL),
        ("/contract/identity_pools/readers/slots/0", BINDING),
        ("/bindings/identity_pools/readers/0", BINDING),
    ] {
        for value in [Value::Null, json!(true), json!(1), json!("object")] {
            let invalid = changed(&base, pointer, value);
            assert_eq!(selection::workload(&invalid, "api", "list", 1), Err(reason));
        }
    }
    for (pointer, reason) in [
        ("/contract/identity_pools/readers", POOL),
        ("/contract/identity_pools/readers/slots/0", BINDING),
        ("/bindings/identity_pools/readers/0", BINDING),
    ] {
        let mut config = base.clone();
        config.pointer_mut(pointer).expect("对象")["extra"] = json!(true);
        assert_eq!(selection::workload(&config, "api", "list", 1), Err(reason));
        let mut config = base.clone();
        let object = config
            .pointer_mut(pointer)
            .expect("对象")
            .as_object_mut()
            .expect("键集合");
        let key = object.keys().next().expect("至少一个键").clone();
        object.remove(&key);
        assert_eq!(selection::workload(&config, "api", "list", 1), Err(reason));
    }
    for key in ["tenant_id", "username", "password_env"] {
        for value in [Value::Null, json!(true), json!(1), json!([]), json!({})] {
            let pointer = format!("/bindings/identity_pools/readers/0/{key}");
            let invalid = changed(&base, &pointer, value);
            assert_eq!(
                selection::workload(&invalid, "api", "list", 1),
                Err(BINDING)
            );
        }
    }
}

#[test]
fn names_and_actual_tenant_and_password_boundaries_are_preserved() {
    let base = config(1, 1, false, &[1]);
    for (field, values) in [
        (
            "tenant_id",
            vec!["".into(), "a".repeat(65), "租户".into(), "tenant\n".into()],
        ),
        (
            "password_env",
            vec![
                "".into(),
                "a".into(),
                "A-".into(),
                "1A".into(),
                "A\n".into(),
            ],
        ),
    ] {
        for value in values {
            let pointer = format!("/bindings/identity_pools/readers/0/{field}");
            let invalid = changed(&base, &pointer, json!(value));
            assert_eq!(
                selection::workload(&invalid, "api", "list", 1),
                Err(BINDING)
            );
        }
    }
    for value in ["A0_-".to_owned(), "a".repeat(64)] {
        let valid = changed(
            &base,
            "/bindings/identity_pools/readers/0/tenant_id",
            json!(value),
        );
        assert!(selection::workload(&valid, "api", "list", 1).is_ok());
    }
    for value in ["A".to_owned(), "A_0".into(), "A".repeat(1000)] {
        let valid = changed(
            &base,
            "/bindings/identity_pools/readers/0/password_env",
            json!(value),
        );
        assert!(selection::workload(&valid, "api", "list", 1).is_ok());
    }
    for key in ["tenant_slot", "user_slot"] {
        for value in [
            "".to_owned(),
            "A".into(),
            "0a".into(),
            "a_".into(),
            "a".repeat(65),
            "a\n".into(),
        ] {
            let pointer = format!("/contract/identity_pools/readers/slots/0/{key}");
            let invalid = changed(&base, &pointer, json!(value));
            assert_eq!(
                selection::workload(&invalid, "api", "list", 1),
                Err(BINDING)
            );
        }
    }
}

#[test]
fn declared_distribution_is_exact_for_the_selected_prefix() {
    let base = config(2, 1, false, &[2]);
    for declared in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"tenant-1": 1}),
        json!({"tenant-1": 2, "tenant-2": 1}),
        json!({"tenant-2": 2}),
        json!({"Tenant": 2}),
        json!({"tenant-1": "2"}),
    ] {
        let config = changed(
            &base,
            "/contract/identity_pools/readers/selections/2",
            declared,
        );
        assert_eq!(
            selection::workload(&config, "api", "list", 2),
            Err(DISTRIBUTION)
        );
    }
    let mut missing = base;
    missing["contract"]["identity_pools"]["readers"]["selections"]
        .as_object_mut()
        .expect("声明")
        .remove("2");
    assert_eq!(
        selection::workload(&missing, "api", "list", 2),
        Err(DISTRIBUTION)
    );
    let uneven = scenario(config(11, 10, false, &[11]), "tenants", "list");
    assert_eq!(
        selection::workload(&uneven, "tenants", "list", 11),
        Err(TEN_TENANTS)
    );
}

#[path = "runtime_selection/contracts.rs"]
mod contracts;
