use serde_json::{Value, json};

use crate::devex::runtime_configuration::{operation_catalog, validate};

const ENDPOINT: &str = "测量端点必须为不含凭据、query 和 fragment 的 HTTP URL";
const WORKFLOWS: &str = "运行时场景集合不完整或重复";
const TIMINGS: &str = "任务场景缺少明确的隔离控制库、服务器身份与 MySQL 客户端绑定";

fn configuration() -> Value {
    let mut workloads = json!({});
    for (suite, names) in [
        (
            "api",
            vec!["list", "filter", "page", "login-refresh", "write"],
        ),
        ("jobs", vec!["export", "import", "message", "schedule"]),
        ("tenants", vec!["list", "write", "export"]),
    ] {
        workloads[suite] = Value::Array(
            names
                .into_iter()
                .map(|name| {
                    let mut workflow = json!({"name": name, "steps": [{"operation": "fixture"}]});
                    if suite == "jobs" {
                        workflow["job"] = json!({"kind": name});
                    }
                    workflow
                })
                .collect(),
        );
    }
    json!({
        "schema_version": 1,
        "contract": {
            "dataset_sha256": "a".repeat(64), "environment_sha256": "b".repeat(64),
            "cycles": 10, "timeout_ms": 1000, "workloads": workloads,
            "pacing": {
                "authority_sha256": "c".repeat(64), "request_interval_ms": 1,
                "sample_prepare_wait_ms": 1, "operation_intervals_ms": {}
            }
        },
        "bindings": {
            "scope_id": "devex-test-scope", "api_url": "http://127.0.0.1:18080",
            "frontend_url": "http://127.0.0.1:4174",
            "metrics_urls": {"api": "http://127.0.0.1:18080/metrics",
                "worker": "http://127.0.0.1:19091/metrics"},
            "database_metrics_urls": ["http://127.0.0.1:19104/metrics"],
            "identity_pools": {"users": []},
            "job_timings": {"control": {}, "mysql_client": "fixture", "server_uuid": "fixture"}
        }
    })
}

fn replaced(pointer: &str, value: Value) -> Value {
    let mut config = configuration();
    *config.pointer_mut(pointer).expect("固定配置字段必须存在") = value;
    config
}

fn rejects(config: &Value, suite: &str, error: &str) {
    assert_eq!(validate(config, suite), Err(error.to_owned()));
}

fn document(paths: Value) -> Value {
    json!({"x-ryframe-api-prefix": {"value": "/api/v1"}, "paths": paths})
}

#[test]
fn complete_suites_and_homepage_are_valid() {
    for suite in ["api", "jobs", "tenants", "homepage"] {
        assert_eq!(validate(&configuration(), suite), Ok(()));
    }
    let mut config = configuration();
    config["contract"]
        .as_object_mut()
        .unwrap()
        .remove("workloads");
    assert_eq!(validate(&config, "homepage"), Ok(()));
    config["contract"]["workloads"] = json!(false);
    assert_eq!(validate(&config, "homepage"), Ok(()));
    rejects(&configuration(), "unknown", WORKFLOWS);
}

#[test]
fn schema_and_objects_are_strict_without_binding_external_fingerprints() {
    for value in [Value::Null, json!([]), json!(true)] {
        rejects(&value, "api", "运行时配置结构无效");
    }
    for key in ["contract", "bindings"] {
        for value in [Value::Null, json!([]), json!(true), json!("object")] {
            let mut config = configuration();
            config[key] = value;
            rejects(&config, "api", "运行时配置结构无效");
        }
    }
    for value in [json!("1"), json!(true), json!([1]), json!(2), Value::Null] {
        rejects(
            &replaced("/schema_version", value),
            "api",
            "运行时配置结构无效",
        );
    }
    let mut config = replaced("/schema_version", json!(1.0));
    config["bindings"]["source_fingerprints"] = json!("由调用层绑定");
    config["extra"] = json!(true);
    assert_eq!(validate(&config, "api"), Ok(()));
}

#[test]
fn scope_matches_ascii_length_and_character_rules() {
    for value in ["a--".to_owned(), "123".to_owned(), "a".repeat(64)] {
        assert_eq!(
            validate(&replaced("/bindings/scope_id", json!(value)), "api"),
            Ok(())
        );
    }
    for value in [
        json!("ab"),
        json!("a".repeat(65)),
        json!("-ab"),
        json!("Abc"),
        json!("a_b"),
        json!("abc\n"),
        json!("中文名"),
        json!(["abc"]),
        Value::Null,
    ] {
        rejects(
            &replaced("/bindings/scope_id", value),
            "api",
            "必须登记隔离 scope",
        );
    }
}

#[test]
fn dataset_and_environment_digests_require_lowercase_hex_strings() {
    for key in ["dataset_sha256", "environment_sha256"] {
        let pointer = format!("/contract/{key}");
        for value in [json!("0".repeat(64)), json!("abcdef0123456789".repeat(4))] {
            assert_eq!(validate(&replaced(&pointer, value), "api"), Ok(()));
        }
        for value in [
            json!(["a".repeat(64)]),
            json!([["a".repeat(64)]]),
            json!("A".repeat(64)),
            json!("g".repeat(64)),
            json!("a".repeat(63)),
            json!(format!("{}\n", "a".repeat(64))),
            json!({}),
            json!(64),
            json!(true),
            Value::Null,
        ] {
            rejects(
                &replaced(&pointer, value),
                "api",
                "缺少固定数据集或硬件、存储、网络与服务参数指纹",
            );
        }
    }
}

#[test]
fn metrics_require_exact_processes_and_nonempty_database_endpoints() {
    let error = "进程指标必须且只能登记 api 与 worker";
    for value in [
        Value::Null,
        json!([]),
        json!({"api": "http://localhost"}),
        json!({"api": "http://localhost", "worker": "http://localhost:1", "scheduler": "http://localhost:2"}),
        json!({"api,worker": "http://localhost"}),
    ] {
        rejects(&replaced("/bindings/metrics_urls", value), "api", error);
    }
    let mut config = configuration();
    config["bindings"]
        .as_object_mut()
        .unwrap()
        .remove("metrics_urls");
    rejects(&config, "api", error);
    for value in [Value::Null, json!([]), json!({}), json!("http://localhost")] {
        rejects(
            &replaced("/bindings/database_metrics_urls", value),
            "api",
            "必须登记隔离 MySQL 实例的连接数指标端点",
        );
    }
    let config = replaced(
        "/bindings/database_metrics_urls",
        json!(["http://localhost/metrics", "http://localhost/metrics"]),
    );
    rejects(&config, "api", "数据库指标端点规范化后不能重复");
}

#[test]
fn normalized_duplicates_are_rejected_in_both_endpoint_sets() {
    for (first, second) in [
        ("http://LOCALHOST:80/metrics", "http://localhost/metrics"),
        ("https://localhost:443/metrics", "https://localhost/metrics"),
        ("http://127.0.0.1/a/../metrics", "http://127.0.0.1/metrics"),
        ("http://127.0.0.1", "http://127.0.0.1/"),
        ("http://127.1/metrics", "http://127.0.0.1/metrics"),
        ("http://[0:0:0:0:0:0:0:1]/metrics", "http://[::1]/metrics"),
        (
            "http://localhost/a/%2e%2e/metrics",
            "http://localhost/metrics",
        ),
        (
            "http://例子.测试/metrics",
            "http://xn--fsqu00a.xn--0zwm56d/metrics",
        ),
    ] {
        let config = replaced(
            "/bindings/metrics_urls",
            json!({"api": first, "worker": second}),
        );
        rejects(&config, "api", "API 与 Worker 指标端点规范化后不能重复");
        let config = replaced("/bindings/database_metrics_urls", json!([first, second]));
        rejects(&config, "api", "数据库指标端点规范化后不能重复");
    }
}

#[test]
fn endpoints_reject_credentials_query_fragment_and_non_urls() {
    for pointer in [
        "/bindings/api_url",
        "/bindings/frontend_url",
        "/bindings/metrics_urls/api",
        "/bindings/metrics_urls/worker",
        "/bindings/database_metrics_urls/0",
    ] {
        for value in [
            json!("https://secret:password@example.test/metrics"),
            json!("http://user@example.test"),
            json!("http://:password@example.test"),
            json!("http://localhost?x=1"),
            json!("http://localhost#fragment"),
            json!("ftp://localhost/metrics"),
            json!("//localhost/metrics"),
            json!("not a url"),
            json!("http://[invalid"),
            json!(["http://localhost"]),
            json!(true),
            json!(42),
            Value::Null,
        ] {
            rejects(&replaced(pointer, value), "api", ENDPOINT);
        }
    }
}

#[test]
fn url_normalization_preserves_source_empty_delimiters_and_endpoint_independence() {
    for value in [
        "http://example.test",
        "https://example.test:443/metrics",
        "http://localhost?",
        "http://localhost#",
        "http://localhost?#",
        "http://@localhost/",
    ] {
        assert_eq!(
            validate(&replaced("/bindings/api_url", json!(value)), "api"),
            Ok(())
        );
    }
    let mut config = configuration();
    config["bindings"]["database_metrics_urls"] =
        json!([config["bindings"]["metrics_urls"]["api"].clone()]);
    assert_eq!(validate(&config, "api"), Ok(()));
    config["bindings"]["metrics_urls"] =
        json!({"api": "http://localhost/", "worker": "http://localhost/?"});
    assert_eq!(validate(&config, "api"), Ok(()));
}

#[test]
fn cycle_and_timeout_bounds_require_safe_json_integers() {
    for (key, low, high, error) in [
        ("cycles", 10, 10_000, "每个场景的请求周期 超出范围"),
        ("timeout_ms", 1000, 300_000, "请求超时 超出范围"),
    ] {
        let pointer = format!("/contract/{key}");
        for value in [json!(low), json!(high), json!(f64::from(low))] {
            assert_eq!(validate(&replaced(&pointer, value), "api"), Ok(()));
        }
        for value in [
            json!(low - 1),
            json!(high + 1),
            json!(-1),
            json!(10.5),
            json!(9_007_199_254_740_992_u64),
            json!(u64::MAX),
            json!("1000"),
            json!(true),
            json!([1000]),
            Value::Null,
        ] {
            rejects(&replaced(&pointer, value), "api", error);
        }
    }
}

#[test]
fn identity_pools_are_nonempty_objects_and_legacy_key_is_forbidden() {
    let error = "缺少独立身份池或仍使用旧 identities 结构";
    for value in [Value::Null, json!({}), json!([]), json!([{}]), json!(true)] {
        rejects(&replaced("/bindings/identity_pools", value), "api", error);
    }
    for value in [Value::Null, json!([]), json!(false)] {
        let mut config = configuration();
        config["bindings"]["identities"] = value;
        rejects(&config, "api", error);
    }
    let pools = json!({"constructor": null, "__proto__": null, "toString": null});
    assert_eq!(
        validate(&replaced("/bindings/identity_pools", pools), "api"),
        Ok(())
    );
}

#[test]
fn workflow_sets_are_complete_unique_and_exact_for_each_suite() {
    for suite in ["api", "jobs", "tenants"] {
        let pointer = format!("/contract/workloads/{suite}");
        let original = configuration().pointer(&pointer).unwrap().clone();
        for value in [Value::Null, json!({}), json!([])] {
            rejects(&replaced(&pointer, value), suite, WORKFLOWS);
        }
        let mut missing = original.clone();
        missing.as_array_mut().unwrap().pop();
        rejects(&replaced(&pointer, missing), suite, WORKFLOWS);
        let mut duplicate = original.clone();
        duplicate[1]["name"] = duplicate[0]["name"].clone();
        rejects(&replaced(&pointer, duplicate), suite, WORKFLOWS);
        for name in [json!("unknown"), json!(["list"]), Value::Null] {
            let mut changed = original.clone();
            changed[0]["name"] = name;
            rejects(&replaced(&pointer, changed), suite, WORKFLOWS);
        }
        let mut reordered = original;
        reordered.as_array_mut().unwrap().reverse();
        assert_eq!(validate(&replaced(&pointer, reordered), suite), Ok(()));
    }
}

#[test]
fn workflow_steps_have_one_to_thirty_entries_without_an_extra_step_schema() {
    for suite in ["api", "jobs", "tenants"] {
        let pointer = format!("/contract/workloads/{suite}/0/steps");
        for value in [json!([null]), Value::Array(vec![json!(false); 30])] {
            assert_eq!(validate(&replaced(&pointer, value), suite), Ok(()));
        }
        for value in [
            json!([]),
            Value::Array(vec![Value::Null; 31]),
            json!({}),
            json!("step"),
            Value::Null,
        ] {
            rejects(
                &replaced(&pointer, value),
                suite,
                "场景必须包含 1 到 30 个实际操作",
            );
        }
    }
}

#[test]
fn jobs_require_matching_kind_for_every_workflow() {
    for index in 0..4 {
        let pointer = format!("/contract/workloads/jobs/{index}/job");
        for value in [
            Value::Null,
            json!({}),
            json!({"kind": "wrong"}),
            json!({"kind": ["export"]}),
            json!(false),
        ] {
            rejects(
                &replaced(&pointer, value),
                "jobs",
                "任务场景必须声明同种类的精确关联任务采集",
            );
        }
    }
    let mut config = configuration();
    config["contract"]["workloads"]["jobs"][0]["job"]["extra"] = json!(true);
    assert_eq!(validate(&config, "jobs"), Ok(()));
}

#[test]
fn truthy_jobs_require_exact_timings_keys_without_validating_binding_values() {
    for value in [
        Value::Null,
        json!([]),
        json!({"control": null, "mysql_client": null}),
        json!({"control": null, "mysql_client": null, "server_uuid": null, "extra": null}),
        json!({"control,mysql_client,server_uuid": null}),
    ] {
        rejects(&replaced("/bindings/job_timings", value), "jobs", TIMINGS);
    }
    let arbitrary = json!({"control": null, "mysql_client": false, "server_uuid": []});
    assert_eq!(
        validate(&replaced("/bindings/job_timings", arbitrary), "jobs"),
        Ok(())
    );
    for suite in ["api", "tenants"] {
        for job in [json!({}), json!([]), json!(true), json!(1), json!("job")] {
            let mut config = replaced("/bindings/job_timings", Value::Null);
            config["contract"]["workloads"][suite][0]["job"] = job;
            rejects(&config, suite, TIMINGS);
        }
        for job in [Value::Null, json!(false), json!(0), json!("")] {
            let mut config = replaced("/bindings/job_timings", Value::Null);
            config["contract"]["workloads"][suite][0]["job"] = job;
            assert_eq!(validate(&config, suite), Ok(()));
        }
    }
}

#[test]
fn pacing_contract_errors_are_delegated() {
    let error = "固定节奏必须声明真实限流摘要、请求/接口间隔与样本准备窗口";
    for value in [Value::Null, json!({}), json!([])] {
        rejects(&replaced("/contract/pacing", value), "homepage", error);
    }
    let mut config = configuration();
    config["contract"]["pacing"]["authority_sha256"] = json!(["c".repeat(64)]);
    rejects(&config, "api", error);
    let mut config = configuration();
    config["contract"]["pacing"]["extra"] = json!(true);
    rejects(&config, "api", error);
}

#[test]
fn catalog_retains_actual_operation_methods_and_absolute_paths() {
    let paths = json!({
        "/api/v1/system/posts": {"get": {"operationId": "get_system_posts"}},
        "/api/v1/common/shell-settings": {"get": {"operationId": "get_common_shell_settings"}},
        "/api/v1/auth/tenants": {"get": {"operationId": "get_auth_tenants"}}
    });
    let catalog = operation_catalog(&document(paths)).unwrap();
    assert_eq!(catalog.len(), 3);
    for (id, path) in [
        ("get_system_posts", "/api/v1/system/posts"),
        ("get_common_shell_settings", "/api/v1/common/shell-settings"),
        ("get_auth_tenants", "/api/v1/auth/tenants"),
    ] {
        assert_eq!(catalog[id], json!({"method": "GET", "path": path}));
    }
}

#[test]
fn catalog_prefix_is_a_string_with_single_leading_slash() {
    for prefix in [
        Value::Null,
        json!(true),
        json!(["/api/v1"]),
        json!(""),
        json!("api/v1"),
        json!("//host/api"),
        json!("https://host/api"),
    ] {
        let mut schema = document(json!({}));
        schema["x-ryframe-api-prefix"]["value"] = prefix;
        assert_eq!(
            operation_catalog(&schema),
            Err("契约缺少明确的 API 前缀".into())
        );
    }
    let mut schema = document(json!({}));
    schema
        .as_object_mut()
        .unwrap()
        .remove("x-ryframe-api-prefix");
    assert_eq!(
        operation_catalog(&schema),
        Err("契约缺少明确的 API 前缀".into())
    );
    for prefix in ["/", "/api/v1?kept#kept"] {
        let mut schema = document(json!({}));
        schema["x-ryframe-api-prefix"]["value"] = json!(prefix);
        assert!(operation_catalog(&schema).unwrap().is_empty());
    }
}

#[test]
fn catalog_rejects_illegal_selected_paths() {
    for route in [
        "",
        "relative",
        "https://host/path",
        "//host/path",
        "///path",
        "/path?query",
        "/path#fragment",
        "/?",
        "/#",
    ] {
        let mut paths = json!({});
        paths[route] = json!({"get": {"operationId": "selected"}});
        assert_eq!(
            operation_catalog(&document(paths)),
            Err("operation 必须声明当前服务的绝对路径".into())
        );
    }
}

#[test]
fn catalog_does_not_expand_path_or_method_contracts() {
    for route in [
        "/",
        "/other-prefix",
        "/internal//segments",
        "/encoded%3F%23",
    ] {
        let mut paths = json!({});
        paths[route] = json!({"custom": {"operationId": "selected"}});
        let catalog = operation_catalog(&document(paths)).unwrap();
        assert_eq!(
            catalog["selected"],
            json!({"method": "CUSTOM", "path": route})
        );
    }
    let ignored = document(json!({"relative?ignored": {"parameters": []}}));
    assert!(operation_catalog(&ignored).unwrap().is_empty());
}

#[test]
fn catalog_rejects_duplicate_ids_and_accepts_prototype_names_as_data() {
    for paths in [
        json!({"/a": {"get": {"operationId": "same"}, "post": {"operationId": "same"}}}),
        json!({"/a": {"get": {"operationId": "same"}}, "/b": {"get": {"operationId": "same"}}}),
        json!({"/a": {"get": {"operationId": "same"}}, "/z?bad": {"get": {"operationId": "same"}}}),
    ] {
        assert_eq!(
            operation_catalog(&document(paths)),
            Err("重复 operation ID".into())
        );
    }
    let schema = document(json!({"/a": {
        "get": {"operationId": "__proto__"}, "post": {"operationId": "constructor"},
        "patch": {"operationId": "toString"}
    }}));
    let catalog = operation_catalog(&schema).unwrap();
    assert_eq!(catalog.len(), 3);
    for (id, method) in [
        ("__proto__", "GET"),
        ("constructor", "POST"),
        ("toString", "PATCH"),
    ] {
        assert_eq!(catalog[id], json!({"method": method, "path": "/a"}));
    }
}

#[test]
fn catalog_skips_falsy_ids_and_rejects_truthy_non_string_ids() {
    for id in [Value::Null, json!(false), json!(0), json!("")] {
        let schema = document(json!({"invalid?ignored": {"get": {"operationId": id}}}));
        assert!(operation_catalog(&schema).unwrap().is_empty());
    }
    for id in [json!(true), json!(1), json!([]), json!({}), json!(["id"])] {
        let schema = document(json!({"/a": {"get": {"operationId": id}}}));
        assert_eq!(
            operation_catalog(&schema),
            Err("operation ID 必须是字符串".into())
        );
    }
}

#[test]
fn malformed_catalog_structures_fail_with_fixed_errors() {
    for paths in [
        Value::Null,
        json!([]),
        json!(true),
        json!({"/a": null}),
        json!({"/a": []}),
        json!({"/a": {"get": null}}),
    ] {
        assert_eq!(
            operation_catalog(&document(paths)),
            Err("operation 目录结构无效".into())
        );
    }
    assert_eq!(
        operation_catalog(&Value::Null),
        Err("契约缺少明确的 API 前缀".into())
    );
}
