use std::{collections::BTreeMap, io::Cursor, time::Duration};

use serde_json::{Value, json};

use crate::devex::job_evidence::{
    protocol::{self, Session},
    wait,
};

fn binding() -> Value {
    json!({"control": {"host": "127.0.0.1", "port": 3306, "database": "devex_control", "username": "test", "password_env": "DEVEX_TEST_PASSWORD", "tls_mode": "required"}, "server_uuid": "12345678-1234-1234-1234-123456789abc", "mysql_client": "fixture-client"})
}

fn selection() -> Value {
    json!({"kind": "export", "id": "101", "tenant_id": "tenant-a", "job_type": "system.export.execute"})
}

fn job() -> Value {
    json!({"business_id": "101", "business_tenant": "tenant-a", "business_status": "succeeded", "job_id": "201", "job_tenant": "tenant-a", "job_type": "system.export.execute", "status": "succeeded", "claim_sequence": 1})
}

fn attempt() -> Value {
    json!({"job_id": "201", "sequence": 1, "outcome": "succeeded", "available_at": "2026-09-04T00:00:00.000000Z", "started_at": "2026-09-04T00:00:01.000000Z", "finished_at": "2026-09-04T00:00:03.000000Z", "closed_at": "2026-09-04T00:00:03.000000Z"})
}

struct Fake {
    queries: Vec<String>,
    scope: String,
    server: String,
    jobs: Vec<Value>,
    attempts: Vec<Value>,
    failure: Option<usize>,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            queries: Vec::new(),
            scope: "devex-test-scope".into(),
            server: binding()["server_uuid"].as_str().unwrap().into(),
            jobs: vec![job()],
            attempts: vec![attempt()],
            failure: None,
        }
    }
}

impl Session for Fake {
    fn query(&mut self, sql: &str) -> Result<Vec<Value>, &'static str> {
        self.queries.push(sql.into());
        if self.failure == Some(self.queries.len()) {
            return Err("database_collection_failed");
        }
        if sql.contains("ryframe_resource_ownership") {
            return Ok(vec![
                json!({"scope": self.scope, "marker": format!("ryframe-owner:v1:{}:control", self.scope), "server": self.server}),
            ]);
        }
        if sql.contains("sys_background_job_attempt") {
            return Ok(self.attempts.clone());
        }
        if sql.starts_with("SELECT JSON_OBJECT") {
            return Ok(self.jobs.clone());
        }
        Ok(Vec::new())
    }
    fn close(&mut self) -> Result<(), &'static str> {
        Ok(())
    }
}

#[test]
fn collection_uses_exact_owner_and_all_attempt_timestamps_in_one_read_only_snapshot() {
    let mut session = Fake::default();
    let proof =
        protocol::collect(&mut session, &binding(), "devex-test-scope", &selection()).unwrap();
    assert_eq!(proof["valid"], true);
    assert_eq!(proof["queue_ms"].as_f64(), Some(1000.0));
    assert_eq!(proof["execution_ms"].as_f64(), Some(2000.0));
    assert_eq!(proof["job"], job());
    assert_eq!(proof["attempts"], json!([attempt()]));
    assert_eq!(session.queries.len(), 5);
    assert_eq!(session.queries[0], "SET SESSION time_zone='+00:00'");
    assert_eq!(session.queries[1], "START TRANSACTION READ ONLY");
    assert!(session.queries[2].contains("ryframe_resource_ownership"));
    assert!(session.queries[3].contains("b.id=101 AND b.tenant_id='tenant-a'"));
    assert!(session.queries[4].contains("WHERE job_id=201 ORDER BY `sequence`"));
    assert!(session.queries.iter().all(|sql| !sql.contains("updated_at")
        && !sql.contains("UPDATE")
        && !sql.contains("DELETE")));
}

#[test]
fn wrong_owner_or_server_stops_before_any_business_read() {
    for (scope, server, reason) in [
        (
            "other-scope",
            "12345678-1234-1234-1234-123456789abc",
            "wrong_database_ownership",
        ),
        ("devex-test-scope", "other-server", "wrong_server_identity"),
    ] {
        let mut session = Fake {
            scope: scope.into(),
            server: server.into(),
            ..Fake::default()
        };
        assert_eq!(
            protocol::collect(&mut session, &binding(), "devex-test-scope", &selection()),
            Err(reason)
        );
        assert_eq!(session.queries.len(), 3);
    }
}

#[test]
fn collection_preserves_unfinished_and_unknown_attempt_evidence() {
    for (status, outcome, reason) in [
        ("running", "succeeded", "job_unfinished"),
        ("dead", "succeeded", "job_failed"),
        ("succeeded", "lease_expired", "attempt_result_unknown"),
    ] {
        let mut session = Fake::default();
        session.jobs[0]["status"] = json!(status);
        session.attempts[0]["outcome"] = json!(outcome);
        let proof =
            protocol::collect(&mut session, &binding(), "devex-test-scope", &selection()).unwrap();
        assert_eq!(proof["valid"], false);
        assert_eq!(proof["reason"], reason);
        assert_eq!(proof["attempts"], json!(session.attempts));
    }
    let mut session = Fake {
        attempts: vec![attempt(); 10_001],
        ..Fake::default()
    };
    assert_eq!(
        protocol::collect(&mut session, &binding(), "devex-test-scope", &selection()),
        Err("attempt_history_limit")
    );
}

#[test]
fn binding_rejects_injection_booleans_missing_secrets_and_remote_hosts() {
    let environment = BTreeMap::from([("DEVEX_TEST_PASSWORD".into(), "fixture-only".into())]);
    protocol::binding(&binding(), "devex-test-scope", &environment).unwrap();
    for (path, invalid) in [
        ("database", json!("x`;DROP")),
        ("host", json!("shared.example.invalid")),
        ("port", json!(true)),
        ("port", json!(65536)),
        ("password_env", json!("APP_TOKEN;")),
        ("username", json!("")),
        ("tls_mode", json!("unknown")),
    ] {
        let mut value = binding();
        value["control"][path] = invalid;
        assert!(protocol::binding(&value, "devex-test-scope", &environment).is_err());
    }
    assert_eq!(
        protocol::binding(&binding(), "devex-test-scope", &BTreeMap::new()),
        Err("missing_database_password_environment")
    );
    assert_eq!(
        protocol::binding(&binding(), "x';--", &environment),
        Err("invalid_scope")
    );
}

#[test]
fn private_input_rejects_duplicates_non_utf8_oversize_and_unterminated_frames() {
    for bytes in [
        br#"{"binding":{},"binding":{}}"#.as_slice(),
        b"\xff",
        b"NaN",
    ] {
        assert!(protocol::parse(bytes, "invalid_collector_request").is_err());
    }
    assert!(protocol::parse(&vec![b' '; 32_769], "invalid_collector_request").is_err());
    assert!(protocol::read_line(&mut Cursor::new(b"{}".as_slice())).is_err());
    assert!(protocol::read_line(&mut Cursor::new(vec![b' '; 32_769])).is_err());
    assert_eq!(
        protocol::read_line(&mut Cursor::new(b"{}\n".as_slice())).unwrap(),
        Some(json!({}))
    );
    assert_eq!(
        protocol::read_line(&mut Cursor::new(Vec::<u8>::new())).unwrap(),
        None
    );
}

#[test]
fn password_environment_lookup_preserves_platform_case_rules() {
    let mut environment = BTreeMap::from([("devex_test_password".into(), "fixture-only".into())]);
    let expected = if cfg!(windows) {
        Ok(())
    } else {
        Err("missing_database_password_environment")
    };
    assert_eq!(
        protocol::binding(&binding(), "devex-test-scope", &environment),
        expected
    );
    environment.insert("DEVEX_TEST_PASSWORD".into(), "another-fixture".into());
    let expected = if cfg!(windows) {
        Err("ambiguous_database_password_environment")
    } else {
        Ok(())
    };
    assert_eq!(
        protocol::binding(&binding(), "devex-test-scope", &environment),
        expected
    );
}

#[test]
fn each_wait_poll_reopens_snapshot_and_always_rolls_back_including_owner_failure() {
    let mut session = Fake::default();
    let mut pending = BTreeMap::new();
    let mut results = Vec::new();
    wait::accept(
        &json!({"ticket": 1, "selection": selection(), "timeout_ms": 1000}),
        &mut pending,
        &mut 0,
        Duration::ZERO,
    )
    .unwrap();
    session.jobs[0]["status"] = json!("running");
    wait::poll(
        &mut session,
        &binding(),
        "devex-test-scope",
        &mut pending,
        || Duration::from_millis(100),
        &mut |value| {
            results.push(value);
            Ok(())
        },
    )
    .unwrap();
    assert!(results.is_empty());
    session.jobs[0]["status"] = json!("succeeded");
    wait::poll(
        &mut session,
        &binding(),
        "devex-test-scope",
        &mut pending,
        || Duration::from_millis(200),
        &mut |value| {
            results.push(value);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["valid"], true);
    assert!(pending.is_empty());
    assert_eq!(
        session
            .queries
            .iter()
            .filter(|sql| *sql == "START TRANSACTION READ ONLY")
            .count(),
        2
    );
    assert_eq!(
        session
            .queries
            .iter()
            .filter(|sql| *sql == "ROLLBACK")
            .count(),
        2
    );
    assert!(
        !session
            .queries
            .iter()
            .any(|sql| sql.contains("sys_background_job_attempt"))
    );
    let mut session = Fake {
        scope: "other-scope".into(),
        ..Fake::default()
    };
    assert_eq!(
        wait::poll(
            &mut session,
            &binding(),
            "devex-test-scope",
            &mut pending,
            || Duration::ZERO,
            &mut |_| Ok(())
        ),
        Err("wrong_database_ownership")
    );
    assert_eq!(session.queries.last().unwrap(), "ROLLBACK");
    assert_eq!(session.queries.len(), 3);
}

#[test]
fn success_after_deadline_is_timeout_with_exact_ticket_scope_and_selection() {
    for times in [[1000, 1000], [500, 1010]] {
        let mut session = Fake::default();
        let mut pending = BTreeMap::new();
        wait::accept(
            &json!({"ticket": 1, "selection": selection(), "timeout_ms": 1000}),
            &mut pending,
            &mut 0,
            Duration::ZERO,
        )
        .unwrap();
        let mut moments = times.into_iter();
        let mut results = Vec::new();
        wait::poll(
            &mut session,
            &binding(),
            "devex-test-scope",
            &mut pending,
            || Duration::from_millis(moments.next().unwrap()),
            &mut |value| {
                results.push(value);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            results,
            vec![
                json!({"ticket": 1, "scope_id": "devex-test-scope", "selection": selection(), "valid": false, "reason": "job_completion_timeout"})
            ]
        );
        assert!(pending.is_empty());
    }
}

#[test]
fn wait_requests_are_bounded_monotonic_and_reject_unknown_or_unsafe_input() {
    let mut pending = BTreeMap::new();
    let mut last = 0;
    for ticket in 1..=100 {
        wait::accept(
            &json!({"ticket": ticket, "selection": selection(), "timeout_ms": 1000}),
            &mut pending,
            &mut last,
            Duration::ZERO,
        )
        .unwrap();
    }
    assert!(
        wait::accept(
            &json!({"ticket": 101, "selection": selection(), "timeout_ms": 1000}),
            &mut pending,
            &mut last,
            Duration::ZERO
        )
        .is_err()
    );
    assert_eq!(pending.len(), 100);
    assert_eq!(last, 100);
    pending.clear();
    for value in [
        json!({"ticket": 100, "selection": selection(), "timeout_ms": 1000}),
        json!({"ticket": true, "selection": selection(), "timeout_ms": 1000}),
        json!({"ticket": 101, "selection": selection(), "timeout_ms": 999}),
        json!({"ticket": 101, "selection": selection(), "timeout_ms": 1000, "unknown": true}),
    ] {
        assert!(wait::accept(&value, &mut pending, &mut last, Duration::ZERO).is_err());
        assert!(pending.is_empty());
        assert_eq!(last, 100);
    }
}
