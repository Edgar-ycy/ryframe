use super::*;

fn output() -> (Vec<Value>, Value) {
    let identities = homepage::visitors(&pool(10)).unwrap();
    let rows = identities
        .iter()
        .enumerate()
        .map(|(index, identity)| {
            json!({
                "cycle":index,"cache":"cold","client_address":identity["client_address"],
                "phase":"measuring","succeeded":true,"session_failure":false,"measured_requests":3,
                "diagnostics":[],"times":{"navigation":10,"lcp":20,"content":30}
            })
        })
        .collect::<Vec<_>>();
    (
        identities,
        json!({"format_version":1,"kind":"ryframe-devex-homepage",
        "cache":"cold","elapsed_ms":2000,"cycles":rows}),
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
    let (identities, mut response) = output();
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
    let (identities, original) = output();
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
