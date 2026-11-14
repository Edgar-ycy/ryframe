use super::*;
use crate::session_pacing::model::{binding, budget_deadline, budget_wait};

#[test]
fn independent_client_first_requests_have_no_global_serial_gate() {
    for count in [10, 50, 100] {
        let mut released = 0;
        for index in 0..count {
            let mut identity = identity();
            identity["username"] = json!(format!("reader-{index}"));
            identity["client_address"] = json!(format!("198.18.10.{}", index + 1));
            let mut pacer = Pacer::new(&pacing(), &authority(), catalog(), &identity).unwrap();
            let plan = pacer.plan(&metadata("get_system_posts"), 0).unwrap();
            assert_eq!(plan.wait_ns, 0);
            assert_eq!(
                plan.keys,
                vec![(format!("ip:198.18.10.{}", index + 1), 1000)]
            );
            pacer.sent(&plan, 0, 0, 0).unwrap();
            released += 1;
        }
        assert_eq!(released, count);
    }
}

#[test]
fn source_nested_csrf_login_and_refresh_timeline_is_preserved() {
    let mut pacer = pacer();
    let mut now = 0;
    let mut waits = Vec::new();
    for name in [
        "get_auth_csrf",
        "post_auth_login",
        "get_auth_csrf",
        "post_auth_refresh",
        "get_auth_csrf",
        "post_auth_login",
    ] {
        let before = now;
        let plan = pacer.plan(&metadata(name), now).unwrap();
        now += plan.wait_ns;
        waits.push(plan.wait_ns / NS_PER_MS);
        pacer.sent(&plan, before, now, now).unwrap();
    }
    assert_eq!(now, ms(13250));
    assert_eq!(waits, [0, 1000, 1250, 1000, 1250, 8750]);
}

#[test]
fn login_uses_all_six_dimensions_and_rule_precedence() {
    let mut authority = authority();
    authority["user"]["enabled"] = json!(true);
    authority["api"]["rules"]["/api/v1/auth/login"] = json!(100);
    authority["api"]["rules"]["POST"] = json!(100);
    let pacer = Pacer::new(&pacing(), &authority, catalog(), &identity()).unwrap();
    let plan = pacer.plan(&metadata("post_auth_login"), 0).unwrap();
    let keys: Vec<_> = plan.keys.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "ip:198.18.10.1",
            "operation:198.18.10.1:post_auth_login",
            "user:[\"system\",\"reader\"]",
            "rule:198.18.10.1:POST /api/v1/auth/login",
            "login:[\"system\",\"reader\"]",
            "login-ip:198.18.10.1"
        ]
    );
    for (removed, expected) in [
        (
            "POST /api/v1/auth/login",
            "rule:198.18.10.1:/api/v1/auth/login",
        ),
        ("/api/v1/auth/login", "rule:198.18.10.1:POST"),
    ] {
        authority["api"]["rules"]
            .as_object_mut()
            .unwrap()
            .remove(removed);
        let pacer = Pacer::new(&pacing(), &authority, catalog(), &identity()).unwrap();
        assert!(
            pacer
                .plan(&metadata("post_auth_login"), 0)
                .unwrap()
                .keys
                .iter()
                .any(|(key, _)| key == expected)
        );
    }
}

#[test]
fn budget_wait_precedes_start_recording_and_does_not_inflate_fixed_duration() {
    let mut pacer = pacer();
    let mut summary = Summary::default();
    let first = pacer.plan(&metadata("post_auth_login"), 0).unwrap();
    let budget_finished = ms(60_000);
    pacer.sent(&first, 0, 0, budget_finished).unwrap();
    summary.fixed("post_auth_login", first.wait_ns, 0);
    summary.budget_wait(60_000);
    summary.budget_duration(budget_finished + ms(5));
    let next = pacer
        .plan(&metadata("post_auth_login"), budget_finished)
        .unwrap();
    assert_eq!(next.wait_ns, ms(12250));
    let receipt = summary.document(&pacing());
    assert_eq!(receipt["measurement"], json!({}));
    assert_eq!(
        receipt["preparation"]["post_auth_login"],
        json!({"requests": 1, "requested_ms": 0, "duration_ms": 0})
    );
    assert_eq!(
        receipt["login_budget"],
        json!({"waits": 1, "requested_ms": 60000, "duration_ms": 60005})
    );
}

#[test]
fn early_wake_keeps_waiting_and_clock_rollback_is_rejected() {
    let mut clock = Wait::new(100, 10).unwrap();
    assert_eq!(clock.remaining(100).unwrap(), Some(10));
    assert_eq!(clock.remaining(100).unwrap(), Some(10));
    assert_eq!(clock.remaining(105).unwrap(), Some(5));
    assert_eq!(category(clock.remaining(104)), "pacing-clock");
    let mut clock = Wait::new(100, 10).unwrap();
    assert_eq!(clock.remaining(110).unwrap(), None);
    assert_eq!(clock.remaining(115).unwrap(), None);
    assert_eq!(category(Wait::new(u64::MAX, 1)), "pacing-clock");
}

#[test]
fn failed_queue_items_leave_starts_unchanged_and_next_item_can_proceed() {
    let mut pacer = pacer();
    let mut invalid = metadata("post_auth_login");
    invalid.method = "GET".into();
    assert_eq!(category(pacer.plan(&invalid, 0)), "pacing-operation");
    let abandoned = pacer.plan(&metadata("post_auth_login"), 0).unwrap();
    assert_eq!(abandoned.wait_ns, 0);
    let next = pacer.plan(&metadata("post_auth_login"), 0).unwrap();
    assert_eq!(next.wait_ns, 0);
    pacer.sent(&next, 0, 0, 0).unwrap();
    assert_eq!(
        pacer.plan(&metadata("post_auth_login"), 0).unwrap().wait_ns,
        ms(12250)
    );
}

#[test]
fn failed_before_send_and_clock_checks_do_not_commit_starts() {
    let mut pacer = pacer();
    let abandoned = pacer.plan(&metadata("post_auth_login"), 100).unwrap();
    assert_eq!(
        category(pacer.sent(&abandoned, 100, 99, 99)),
        "pacing-clock"
    );
    let next = pacer.plan(&metadata("post_auth_login"), 100).unwrap();
    assert_eq!(next.wait_ns, 0);
    pacer.sent(&next, 100, 100, 120).unwrap();
    assert_eq!(
        category(pacer.plan(&metadata("post_auth_login"), 119)),
        "pacing-clock"
    );
    let later = pacer.plan(&metadata("post_auth_login"), 120).unwrap();
    assert_eq!(later.wait_ns, ms(12250));
    assert_eq!(
        category(pacer.sent(&later, 120, ms(12250) + 119, ms(12250) + 119)),
        "pacing-clock"
    );
}

#[test]
fn identity_is_immutable_and_only_explicit_benchmark_addresses_are_accepted() {
    let mut identity = identity();
    identity["username"] = json!("\u{feff} READER\u{00a0}");
    let pacer = Pacer::new(&pacing(), &authority(), catalog(), &identity).unwrap();
    identity["client_address"] = json!("198.18.10.2");
    let plan = pacer.plan(&metadata("post_auth_login"), 0).unwrap();
    assert!(
        plan.keys
            .iter()
            .any(|(key, _)| key == "login:[\"system\",\"reader\"]")
    );
    assert!(plan.keys.iter().any(|(key, _)| key == "ip:198.18.10.1"));
    for address in ["198.18.0.0", "198.19.255.255", "198.18.01.00"] {
        identity["client_address"] = json!(address);
        assert!(Pacer::new(&pacing(), &authority(), catalog(), &identity).is_ok());
    }
    for address in [
        "127.0.0.1",
        "198.20.1.1",
        "198.18.256.1",
        "198.18.001.1",
        "198.18.1.1\n",
        "",
    ] {
        identity["client_address"] = json!(address);
        assert_eq!(
            category(Pacer::new(&pacing(), &authority(), catalog(), &identity)),
            "pacing-identity"
        );
    }
}

#[test]
fn binding_requires_only_an_absolute_login_budget_state() {
    let fixture = Fixture::new();
    let good = &fixture.config["bindings"]["pacing"];
    assert_eq!(binding(good).unwrap(), fixture.budget());
    for value in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"extra": fixture.budget()}),
        json!({"login_budget_state": "relative"}),
        json!({"login_budget_state": [fixture.budget()]}),
        json!({"login_budget_state": fixture.budget(), "extra": null}),
        json!({"login_budget_state": fixture.budget(), "python": fixture.root()}),
    ] {
        assert_eq!(category(binding(&value)), "pacing-binding");
    }
    for control in ['\r', '\n', '\0'] {
        let state = format!("{}{control}", fixture.budget().display());
        assert_eq!(
            category(binding(&json!({"login_budget_state": state}))),
            "pacing-binding"
        );
    }
}

#[test]
fn budget_deadline_is_bounded_and_zero_wait_cannot_spin() {
    let deadline = budget_deadline(100, 60000).unwrap();
    assert_eq!(deadline, ms(210000) + 100);
    assert!(budget_wait(100, 210000, deadline).is_ok());
    for (now, requested) in [(100, 210001), (deadline, 1), (100, 0), (u64::MAX, 1)] {
        assert_eq!(
            category(budget_wait(now, requested, deadline)),
            "pacing-budget-timeout"
        );
    }
    assert_eq!(category(budget_deadline(1, u64::MAX)), "pacing-clock");
}

#[test]
fn submillisecond_starts_are_not_rounded_down_or_released_early() {
    let mut pacer = pacer();
    let first = pacer.plan(&metadata("post_auth_login"), 900_000).unwrap();
    pacer.sent(&first, 900_000, 900_000, 900_000).unwrap();
    let next = pacer.plan(&metadata("post_auth_login"), ms(12250)).unwrap();
    assert_eq!(next.wait_ns, 900_000);
    assert_eq!(
        crate::session_pacing::model::milliseconds(next.wait_ns),
        json!(0.9)
    );
}

#[test]
fn source_authority_failures_are_still_rejected_by_shared_validation() {
    let base = pacing();
    for (key, value) in [
        ("request_interval_ms", json!(600)),
        ("sample_prepare_wait_ms", json!(60000)),
    ] {
        let mut pacing = base.clone();
        pacing[key] = value;
        assert_eq!(
            category(Pacer::new(&pacing, &authority(), catalog(), &identity())),
            "pacing-contract"
        );
    }
    for (operation, value) in [
        ("get_auth_csrf", json!(2000)),
        ("post_auth_login", json!(12000)),
        ("post_auth_password_reset_complete", json!(1000)),
        ("unknown", json!(1000)),
    ] {
        let mut pacing = base.clone();
        pacing["operation_intervals_ms"][operation] = value;
        assert_eq!(
            category(Pacer::new(&pacing, &authority(), catalog(), &identity())),
            "pacing-contract"
        );
    }
    let mut authority = authority();
    authority["user"] = json!({"enabled": true, "capacity": 10, "window_ms": 60000});
    assert_eq!(
        category(Pacer::new(&base, &authority, catalog(), &identity())),
        "pacing-contract"
    );
    let mut conservative = base;
    conservative["request_interval_ms"] = json!(6500);
    assert!(Pacer::new(&conservative, &authority, catalog(), &identity()).is_ok());
    authority["api"]["rules"]["/unmatched"] = json!(1);
    assert_eq!(
        category(Pacer::new(
            &conservative,
            &authority,
            catalog(),
            &identity()
        )),
        "pacing-contract"
    );
}
