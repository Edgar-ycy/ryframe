use super::*;

fn binding() -> Binding {
    Binding::new("login-test", 5, 60_000).unwrap()
}

fn keys() -> [String; 2] {
    login_keys("system", "admin", "198.18.1.2").unwrap()
}

fn ledger() -> Ledger {
    Ledger::empty(binding(), 1000).unwrap()
}

fn reserve(state: &mut Ledger, now: u64) -> Vec<Reservation> {
    state
        .reserve(&binding(), &keys(), now)
        .unwrap()
        .reservation
        .unwrap()
}

fn attempt(state: &mut Ledger, selected: &[String; 2], now: u64) {
    let reservation = state
        .reserve(&binding(), selected, now)
        .unwrap()
        .reservation
        .unwrap();
    state.complete(&binding(), &reservation, now).unwrap();
}

#[test]
fn product_scope_capacity_and_window_boundaries_match_the_browser() {
    for scope in [
        "a1".to_owned(),
        format!("a{}z", "_".repeat(46)),
        "dev_local-01".into(),
    ] {
        Binding::new(&scope, 1, 1).unwrap();
        Binding::new(&scope, 10_000, 86_400_000).unwrap();
    }
    for scope in [
        "a".to_owned(),
        format!("a{}z", "_".repeat(47)),
        "Upper".into(),
        "scope.dot".into(),
        "-x".into(),
        "x-".into(),
        "中文".into(),
    ] {
        assert_eq!(Binding::new(&scope, 5, 60_000), Err(BudgetError::Scope));
    }
    for capacity in [0, 10_001, u64::MAX] {
        assert_eq!(Binding::new("a1", capacity, 1), Err(BudgetError::Capacity));
    }
    for window in [0, 86_400_001, u64::MAX] {
        assert_eq!(Binding::new("a1", 1, window), Err(BudgetError::Window));
    }
}

#[test]
fn node_binding_order_is_preserved_and_wrong_order_or_extra_fields_are_rejected() {
    let expected = r#"{"scope":"login-test","capacity":5,"windowMs":60000}"#;
    assert_eq!(serde_json::to_string(&binding()).unwrap(), expected);
    assert_eq!(
        serde_json::from_str::<Binding>(expected).unwrap(),
        binding()
    );
    for text in [
        r#"{"capacity":5,"scope":"login-test","windowMs":60000}"#,
        r#"{"scope":"login-test","capacity":5,"windowMs":60000,"other":true}"#,
        r#"{"scope":"login-test","capacity":"5","windowMs":60000}"#,
    ] {
        assert!(serde_json::from_str::<Binding>(text).is_err());
    }
    assert_eq!(
        serde_json::from_str::<Binding>(r#"{"scope":"login-test","capacity":5.0,"windowMs":6e4}"#)
            .unwrap(),
        binding()
    );
}

#[test]
fn node_generated_hash_vectors_include_unicode_trim_case_and_json_escaping() {
    let vectors: Value = serde_json::from_str(include_str!("node-vectors.json")).unwrap();
    for vector in vectors["cases"].as_array().unwrap() {
        let actual = login_keys(
            vector["tenantId"].as_str().unwrap(),
            vector["username"].as_str().unwrap(),
            vector["address"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(serde_json::to_value(actual).unwrap(), vector["keys"]);
    }
    assert_eq!(
        login_keys("system", "\u{feff} ADMIN \u{a0}", "198.18.1.2").unwrap(),
        keys()
    );
    assert_ne!(
        login_keys("SYSTEM", "admin", "198.18.1.2").unwrap()[0],
        keys()[0]
    );
    assert_ne!(
        login_keys("system", "\u{85}admin", "198.18.1.2").unwrap()[0],
        keys()[0]
    );
    // 地址保留前端正则语义，不额外规范化八位组或前导零。
    login_keys("system", "admin", "198.19.000.999").unwrap();
    for (tenant, user, address) in [
        ("", "admin", "198.18.1.2"),
        ("system", " \u{feff}", "198.18.1.2"),
        ("system", "admin", "127.0.0.1"),
        ("system", "admin", "198.18.1.2\n"),
        ("system", "admin", "198.18.0000.1"),
        ("system", "admin", "198.18.-1.2"),
    ] {
        assert_eq!(
            login_keys(tenant, user, address),
            Err(BudgetError::Identity)
        );
    }
}

#[test]
fn node_ledger_and_reservations_round_trip_without_changing_the_binding_order() {
    let state: Ledger = serde_json::from_str(include_str!("node-ledger.json")).unwrap();
    state.validate(&binding(), 1000).unwrap();
    let encoded = serde_json::to_string(&state).unwrap();
    assert!(encoded.starts_with(
        "{\"version\":1,\"binding\":{\"scope\":\"login-test\",\"capacity\":5,\"windowMs\":60000},"
    ));
    let round_trip: Ledger = serde_json::from_str(&encoded).unwrap();
    round_trip.validate(&binding(), 1000).unwrap();
    let vectors: Value = serde_json::from_str(include_str!("node-vectors.json")).unwrap();
    let reservation: Vec<Reservation> =
        serde_json::from_value(vectors["reservation"].clone()).unwrap();
    assert_eq!(
        serde_json::to_value(&reservation).unwrap(),
        vectors["reservation"]
    );
    let mut completed = state;
    completed.complete(&binding(), &reservation, 2000).unwrap();
    assert!(
        completed
            .buckets
            .values()
            .all(|bucket| bucket.count == 1 && bucket.completed_at == Some(2000))
    );
}

#[test]
fn fifth_attempt_is_immediate_and_sixth_waits_without_refunding_failures() {
    let mut state = ledger();
    for _ in 0..5 {
        // HTTP 成功和 4xx 都由同一个 complete 接口确认，不传入退款条件。
        attempt(&mut state, &keys(), 1000);
    }
    assert_eq!(
        state.reserve(&binding(), &keys(), 1000).unwrap(),
        ReserveOutcome {
            wait_ms: 60_250,
            reservation: None
        }
    );
    assert!(state.buckets.values().all(|bucket| bucket.count == 5));
    assert!(!serde_json::to_string(&state).unwrap().contains("admin"));
    assert_eq!(state.buckets.len(), 2);
}

#[test]
fn late_response_extends_the_whole_batch_and_uncompleted_reservations_still_count() {
    let mut state = ledger();
    for _ in 0..4 {
        attempt(&mut state, &keys(), 1000);
    }
    let pending = reserve(&mut state, 1000);
    assert_eq!(
        state.reserve(&binding(), &keys(), 3000).unwrap().wait_ms,
        58_250
    );
    state.complete(&binding(), &pending, 3000).unwrap();
    assert_eq!(
        state.reserve(&binding(), &keys(), 3000).unwrap().wait_ms,
        60_250
    );
    assert!(state.buckets.values().all(|bucket| bucket.count == 5));
}

#[test]
fn principal_across_clients_and_ip_across_users_share_the_existing_buckets() {
    let mut state = ledger();
    for index in 0..5 {
        let selected = login_keys("system", " ADMIN ", &format!("198.18.1.{index}")).unwrap();
        attempt(&mut state, &selected, 1000);
    }
    let selected = login_keys("system", "admin", "198.18.2.1").unwrap();
    let before = state.buckets.len();
    assert_eq!(
        state.reserve(&binding(), &selected, 1000).unwrap().wait_ms,
        60_250
    );
    assert_eq!(state.buckets.len(), before);
    for index in 0..5 {
        let selected = login_keys("other", &format!("user-{index}"), "198.18.3.1").unwrap();
        attempt(&mut state, &selected, 61_250);
    }
    let selected = login_keys("other", "next", "198.18.3.1").unwrap();
    assert_eq!(
        state
            .reserve(&binding(), &selected, 61_250)
            .unwrap()
            .wait_ms,
        60_250
    );
}

#[test]
fn blocked_result_uses_the_longer_expiry_and_does_not_partly_consume_either_key() {
    let mut state = ledger();
    for _ in 0..5 {
        reserve(&mut state, 1000);
    }
    state.buckets.get_mut(&keys()[0]).unwrap().completed_at = Some(3000);
    state.buckets.get_mut(&keys()[1]).unwrap().completed_at = Some(5000);
    assert_eq!(
        state.reserve(&binding(), &keys(), 5000).unwrap().wait_ms,
        60_250
    );
    assert!(state.buckets.values().all(|bucket| bucket.count == 5));
    assert_eq!(state.observed_at, 5000);
}

#[test]
fn expiry_prunes_old_keys_replaces_generation_and_refuses_old_completions() {
    let mut state = ledger();
    let old = reserve(&mut state, 1000);
    let fresh = reserve(&mut state, 61_250);
    assert!(
        old.iter()
            .zip(&fresh)
            .all(|(old, new)| old.generation != new.generation)
    );
    let before = serde_json::to_vec(&state).unwrap();
    assert_eq!(
        state.complete(&binding(), &old, 61_250),
        Err(BudgetError::Reservation)
    );
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    let next = login_keys("next", "owner", "198.18.9.9").unwrap();
    attempt(&mut state, &next, 121_500);
    assert_eq!(state.buckets.len(), 2);
    assert!(next.iter().all(|key| state.buckets.contains_key(key)));
}

#[test]
fn invalid_completion_does_not_partly_change_an_earlier_valid_entry() {
    let mut state = ledger();
    let mut pending = reserve(&mut state, 1000);
    pending[1].generation = "00000000-0000-0000-0000-000000000000".into();
    let before = serde_json::to_vec(&state).unwrap();
    assert_eq!(
        state.complete(&binding(), &pending, 2000),
        Err(BudgetError::Reservation)
    );
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
}

#[test]
fn active_bucket_limit_never_evicts_a_live_budget_and_expired_buckets_are_reclaimed() {
    let mut state = ledger();
    for index in 0..1023 {
        state.buckets.insert(
            format!("ip:{index:064x}"),
            Bucket {
                generation: "00000000-0000-4000-8000-000000000001".into(),
                count: 1,
                reserved_at: 1000,
                completed_at: None,
            },
        );
    }
    assert_eq!(
        state.reserve(&binding(), &keys(), 1000),
        Err(BudgetError::BucketsFull)
    );
    assert_eq!(state.buckets.len(), 1023);
    state.buckets.remove(&format!("ip:{:064x}", 0));
    reserve(&mut state, 1000);
    assert_eq!(state.buckets.len(), 1024);
    let selected = login_keys("another", "owner", "198.19.3.4").unwrap();
    assert_eq!(
        state.reserve(&binding(), &selected, 1000),
        Err(BudgetError::BucketsFull)
    );
    attempt(&mut state, &selected, 61_250);
    assert_eq!(state.buckets.len(), 2);
}

#[test]
fn damaged_entries_future_times_and_observed_clock_rollback_fail_closed() {
    let mut state = ledger();
    reserve(&mut state, 1000);
    let original = state.clone();
    state.observed_at = 1001;
    assert_eq!(state.validate(&binding(), 1000), Err(BudgetError::Ledger));
    state = original.clone();
    state.buckets.get_mut(&keys()[0]).unwrap().reserved_at = 1001;
    assert_eq!(state.validate(&binding(), 1000), Err(BudgetError::Bucket));
    state = original.clone();
    state.buckets.get_mut(&keys()[0]).unwrap().completed_at = Some(1001);
    assert_eq!(state.validate(&binding(), 1000), Err(BudgetError::Bucket));
    for count in [0, 6] {
        state = original.clone();
        state.buckets.get_mut(&keys()[0]).unwrap().count = count;
        assert_eq!(state.validate(&binding(), 1000), Err(BudgetError::Bucket));
    }
    state = original.clone();
    state.buckets.get_mut(&keys()[0]).unwrap().generation = "A".repeat(36);
    assert_eq!(state.validate(&binding(), 1000), Err(BudgetError::Bucket));
    state = original;
    let bucket = state.buckets.values().next().unwrap().clone();
    state.buckets.insert("invalid".into(), bucket);
    assert_eq!(state.validate(&binding(), 1000), Err(BudgetError::Bucket));
    assert_eq!(
        Ledger::empty(binding(), MAX_SAFE_INTEGER + 1).unwrap_err(),
        BudgetError::Clock
    );
}

#[test]
fn parser_preserves_js_safe_integer_acceptance_and_rejects_missing_times() {
    let source = include_str!("node-ledger.json");
    for (from, to) in [
        ("\"count\":1", "\"count\":1.0"),
        ("\"observedAt\":1000", "\"observedAt\":1e3"),
    ] {
        serde_json::from_str::<Ledger>(&source.replace(from, to))
            .unwrap()
            .validate(&binding(), 1000)
            .unwrap();
    }
    for (from, to) in [
        ("\"count\":1", "\"count\":0.5"),
        ("\"reservedAt\":1000", "\"reservedAt\":-1"),
        ("\"observedAt\":1000", "\"observedAt\":9007199254740992"),
        ("\"completedAt\":1000", "\"completedAt\":\"private\""),
        (",\"completedAt\":1000", ""),
    ] {
        assert!(serde_json::from_str::<Ledger>(&source.replace(from, to)).is_err());
    }
}

#[test]
fn javascript_expiry_rounding_is_retained_near_the_safe_integer_boundary() {
    let binding = Binding::new("a1", 1, 86_400_000).unwrap();
    let mut state = Ledger::empty(binding.clone(), MAX_SAFE_INTEGER).unwrap();
    state.reserve(&binding, &keys(), MAX_SAFE_INTEGER).unwrap();
    let blocked = state.reserve(&binding, &keys(), MAX_SAFE_INTEGER).unwrap();
    assert_eq!(blocked.wait_ms, 86_400_251);
}
