use super::*;

fn output(cache: &str) -> (Vec<Value>, Value) {
    let identities = homepage::visitors(&pool(10)).unwrap();
    let static_cache = if cache == "warm" {
        json!({"responses":3,"hits":2,"primed_hits":1})
    } else {
        json!({"responses":3,"hits":0,"primed_hits":0})
    };
    let rows = identities
        .iter()
        .enumerate()
        .map(|(index, identity)| {
            json!({
                "cycle":index,"cache":cache,"client_address":identity["client_address"],
                "phase":"measuring","succeeded":true,"session_failure":false,"measured_requests":3,
                "static_cache":static_cache,
                "diagnostics":[],"times":{"navigation":10,"lcp":20,"content":30}
            })
        })
        .collect::<Vec<_>>();
    (
        identities,
        json!({"format_version":1,"kind":"ryframe-devex-homepage",
        "cache":cache,"elapsed_ms":2000,"cycles":rows}),
    )
}

#[test]
fn homepage_input_preserves_order_and_rejects_invalid_modes_addresses_and_dimensions() {
    let original = pool(10);
    let identities = homepage::visitors(&original).unwrap();
    for (index, identity) in identities.iter().enumerate() {
        assert_eq!(
            identity["username"],
            original["bindings"]["identity_pools"]["system"][index]["username"]
        );
        assert_eq!(identity["client_address"], format!("198.18.0.{index}"));
    }
    assert_eq!(homepage::visitors(&original).unwrap(), identities);
    for (pointer, value) in [
        ("/contract/cycles", json!(9)),
        ("/contract/cycles", json!(10001)),
        ("/contract/homepage", Value::Null),
        ("/contract/homepage/visitor_interval_ms", json!(-1)),
        ("/contract/homepage/visitor_interval_ms", json!(60001)),
        ("/contract/homepage/visitor_interval_ms", json!(1.5)),
        ("/bindings/api_url", json!("https://example.test")),
        ("/bindings/frontend_url", json!("https://example.test")),
        (
            "/bindings/frontend_url",
            json!("http://user:secret@127.0.0.1:4174"),
        ),
        (
            "/bindings/identity_pools/system/1/username",
            json!("bench-0"),
        ),
        (
            "/contract/identity_pools/system/slots/1/client_address",
            json!("198.18.0.0"),
        ),
        (
            "/contract/identity_pools/system/slots/1/client_address",
            json!("198.18.999.1"),
        ),
    ] {
        let mut invalid = original.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        assert!(homepage::visitors(&invalid).is_err(), "{pointer}");
    }
    let mut incomplete = original.clone();
    incomplete["bindings"]["identity_pools"]["system"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(homepage::visitors(&incomplete).is_err());
    for interval in [0, 60000] {
        let mut valid = original.clone();
        valid["contract"]["homepage"]["visitor_interval_ms"] = json!(interval);
        assert_eq!(homepage::visitors(&valid).unwrap(), identities);
    }
}

#[test]
fn homepage_summaries_keep_nearest_rank_and_count_only_login_as_session_failure() {
    let (identities, mut response) = output("cold");
    for (index, duration) in [99, 1, 10, 20].into_iter().enumerate() {
        response["cycles"][index]["times"] =
            json!({"navigation":duration,"lcp":duration,"content":duration});
    }
    for (index, phase) in [
        "preparing",
        "login",
        "warming",
        "measuring",
        "preparing",
        "login",
    ]
    .into_iter()
    .enumerate()
    {
        let row = &mut response["cycles"][index + 4];
        row["succeeded"] = json!(false);
        row["phase"] = json!(phase);
        row["session_failure"] = json!(phase == "login");
        row["failure"] = json!("timeout");
        row["measured_requests"] = json!(if phase == "measuring" { 3 } else { 0 });
        row["static_cache"] = json!({"responses":0,"hits":0,"primed_hits":0});
    }
    let measured = homepage::validate(&response, 10, "cold", &identities).unwrap();
    assert_eq!(measured.completed_cycles, 4);
    assert_eq!(measured.failed_cycles, 6);
    assert_eq!(measured.session_failures, 2);
    assert_eq!(measured.network_requests, 15);
    for summary in measured.scenarios.values() {
        assert_eq!(
            summary,
            &json!({"completed":4,"failed":6,"elapsed_ms":2000.0,
            "p50_ms":10.0,"p95_ms":99.0,"p99_ms":99.0,"throughput":2.0,
            "queue_p95_ms":null,"execution_p95_ms":null})
        );
    }
    response["elapsed_ms"] = json!(0);
    for row in response["cycles"].as_array_mut().unwrap() {
        row["succeeded"] = json!(false);
        row["phase"] = json!("warming");
        row["session_failure"] = json!(false);
        row["failure"] = json!("browser_validation_failed");
        row["measured_requests"] = json!(0);
    }
    let failed = homepage::validate(&response, 10, "cold", &identities).unwrap();
    assert_eq!(failed.completed_cycles, 0);
    assert_eq!(failed.failed_cycles, 10);
    assert_eq!(failed.session_failures, 0);
    assert!(
        failed
            .scenarios
            .values()
            .all(|row| row["p95_ms"] == 0.0 && row["throughput"] == 0.0)
    );
}

#[test]
fn homepage_output_rejects_rebinding_private_fields_and_untrusted_failure_categories() {
    let (identities, original) = output("cold");
    for (pointer, value) in [
        ("/cache", json!("warm")),
        ("/cycles/0/cycle", json!(1)),
        ("/cycles/0/client_address", json!("198.18.0.1")),
        ("/cycles/0/session_failure", json!(true)),
        ("/cycles/0/times/lcp", json!(0)),
        ("/cycles/0/times/content", json!(-1)),
        ("/cycles/0/measured_requests", json!(0)),
        ("/cycles/0/diagnostics", json!(["homepage_page_error"])),
    ] {
        let mut invalid = original.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        assert!(
            homepage::validate(&invalid, 10, "cold", &identities).is_err(),
            "{pointer}"
        );
    }
    let mut private = original.clone();
    private["cycles"][0]["password"] = json!("private-password");
    assert_eq!(
        homepage::validate(&private, 10, "cold", &identities)
            .err()
            .unwrap()
            .category,
        "homepage_cycle_binding"
    );
    let mut failed = original;
    failed["cycles"][0]["succeeded"] = json!(false);
    failed["cycles"][0]["failure"] = json!("private-password");
    let error = homepage::validate(&failed, 10, "cold", &identities)
        .err()
        .unwrap();
    assert_eq!(error.category, "homepage_failure_category");
    assert!(!error.document().to_string().contains("private-password"));
}

#[test]
fn homepage_static_cache_rejects_missing_malformed_and_unqualified_observations() {
    let (identities, cold) = output("cold");
    let (_, warm) = output("warm");
    let cases = [
        ("missing", &cold, None),
        ("null", &cold, Some(Value::Null)),
        ("array", &cold, Some(json!([3, 0, 0]))),
        (
            "extra field",
            &cold,
            Some(json!({"responses":3,"hits":0,"primed_hits":0,"private":"do not report"})),
        ),
        (
            "wrong key",
            &cold,
            Some(json!({"responses":3,"hits":0,"other":0})),
        ),
        (
            "cold without responses",
            &cold,
            Some(json!({"responses":0,"hits":0,"primed_hits":0})),
        ),
        (
            "cold with hit",
            &cold,
            Some(json!({"responses":3,"hits":1,"primed_hits":0})),
        ),
        (
            "warm without primed hit",
            &warm,
            Some(json!({"responses":3,"hits":1,"primed_hits":0})),
        ),
        (
            "hits exceed responses",
            &warm,
            Some(json!({"responses":1,"hits":2,"primed_hits":1})),
        ),
        (
            "primed hits exceed hits",
            &warm,
            Some(json!({"responses":3,"hits":1,"primed_hits":2})),
        ),
    ];
    let reject = |response: &Value, case: &str| {
        let cache = response["cache"].as_str().unwrap();
        let error = homepage::validate(response, 10, cache, &identities)
            .err()
            .unwrap_or_else(|| panic!("accepted {case}"));
        assert_eq!(
            error,
            failure::Failure::new("homepage_cache_evidence"),
            "{case}"
        );
    };
    for (case, original, observation) in cases {
        let mut invalid = original.clone();
        let row = invalid["cycles"][0].as_object_mut().unwrap();
        match observation {
            Some(value) => {
                row.insert("static_cache".into(), value);
            }
            None => {
                row.remove("static_cache");
            }
        }
        reject(&invalid, case);
    }
    let beyond_u64: Value = serde_json::from_str("18446744073709551616").unwrap();
    for field in ["responses", "hits", "primed_hits"] {
        for value in [
            json!(-1),
            json!(1.5),
            json!(1.0),
            json!("1"),
            json!(true),
            beyond_u64.clone(),
        ] {
            let mut invalid = warm.clone();
            invalid["cycles"][0]["static_cache"][field] = value;
            reject(&invalid, field);
        }
    }
}

#[test]
fn homepage_warm_cycles_require_primed_hits_only_when_successful() {
    let (identities, mut response) = output("warm");
    let failed = &mut response["cycles"][0];
    failed["succeeded"] = json!(false);
    failed["phase"] = json!("warming");
    failed["failure"] = json!("browser_validation_failed");
    failed["measured_requests"] = json!(0);
    failed["static_cache"] = json!({"responses":0,"hits":0,"primed_hits":0});
    let measured = homepage::validate(&response, 10, "warm", &identities).unwrap();
    assert_eq!(measured.completed_cycles, 9);
    assert_eq!(measured.failed_cycles, 1);
    assert_eq!(measured.session_failures, 0);
    assert_eq!(measured.network_requests, 27);

    response["cycles"][0]["static_cache"]["hits"] = json!(1);
    assert_eq!(
        homepage::validate(&response, 10, "warm", &identities)
            .err()
            .unwrap()
            .category,
        "homepage_cache_evidence"
    );
}

#[test]
fn missing_frontend_browser_tool_fails_without_falling_back_to_backend_javascript() {
    asynchronous(async {
        let temp = tempfile::tempdir().unwrap();
        let mut input = input(temp.path());
        input.suite = "homepage".into();
        input.concurrency = 0;
        let error = homepage::measure(
            &pool(10),
            &input,
            temp.path(),
            &process::Cancellation::default(),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error.category, "frontend_homepage_tool_missing");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    });
}
