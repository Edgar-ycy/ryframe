//! 任务证据纯模型与固定 SQL 的回归测试。

use crate::devex::job_evidence::{model, sql};

use chrono::SecondsFormat;
use serde_json::{Value, json};

fn selection() -> Value {
    json!({"kind": "export", "id": "101", "tenant_id": "tenant-a",
        "job_type": "system.export.execute"})
}

fn job() -> Value {
    json!({"business_id": "101", "business_tenant": "tenant-a", "business_status": "succeeded",
        "job_id": "201", "job_tenant": "tenant-a", "job_type": "system.export.execute",
        "status": "succeeded", "claim_sequence": 1})
}

fn attempt() -> Value {
    json!({"job_id": "201", "sequence": 1, "outcome": "succeeded",
        "available_at": "2026-09-04T00:00:00.000000Z",
        "started_at": "2026-09-04T00:00:01.000000Z",
        "finished_at": "2026-09-04T00:00:03.000000Z",
        "closed_at": "2026-09-04T00:00:03.000000Z"})
}

fn changed(mut value: Value, key: &str, replacement: Value) -> Value {
    value[key] = replacement;
    value
}

fn schedule() -> Value {
    let mut expected = selection();
    expected["kind"] = json!("schedule");
    expected["tenant_id"] = json!("system");
    expected["job_type"] = json!("system.message.retention");
    expected
}

fn message() -> Value {
    let mut expected = selection();
    expected["kind"] = json!("message");
    expected["job_type"] = json!("system.message.dispatch");
    expected
}

fn platform_job() -> Value {
    let mut row = job();
    row["business_tenant"] = json!("system");
    row["business_status"] = json!("enqueued");
    row["job_tenant"] = Value::Null;
    row["job_type"] = json!("system.message.retention");
    row
}

fn assert_reason(row: &Value, attempts: &[Value], reason: &str) {
    let result = model::evidence(row, attempts);
    assert_eq!(result["valid"], false);
    assert_eq!(result["reason"], reason);
    assert_eq!(&result["job"], row);
    assert_eq!(result["attempts"], json!(attempts));
    assert!(result.get("queue_ms").is_none());
    assert!(result.get("execution_ms").is_none());
}

// Python: test_success_uses_exact_owner_job_and_persisted_times 的纯模型、SQL 部分。
#[test]
fn success_uses_exact_job_and_persisted_times() {
    let row = model::associated_job(&[job()], &selection()).expect("精确关联");
    let result = model::evidence(&row, &[attempt()]);
    assert_eq!(result["valid"], true);
    assert_eq!(result["queue_ms"].as_f64(), Some(1000.0));
    assert_eq!(result["execution_ms"].as_f64(), Some(2000.0));
    assert_eq!(result["attempt_count"], 1);
    assert_eq!(result["unsuccessful_attempts"], 0);
    assert!(result.get("reason").is_none());
    let job_sql = sql::job_sql("devex_control", &selection()).expect("固定任务查询");
    let attempts_sql = sql::attempts_sql("devex_control", "201").expect("固定尝试查询");
    assert!(job_sql.contains("b.id=101 AND b.tenant_id='tenant-a'"));
    assert!(attempts_sql.contains("WHERE job_id=201 ORDER BY `sequence`"));
    for query in [job_sql, attempts_sql] {
        assert!(query.starts_with("SELECT JSON_OBJECT("));
        for forbidden in ["UPDATE", "DELETE", "updated_at"] {
            assert!(!query.contains(forbidden));
        }
    }
}

// Python: test_wrong_scope_server_tenant_type_and_job_are_rejected 的关联部分。
#[test]
fn wrong_tenant_type_and_job_are_rejected() {
    for (key, value, reason) in [
        ("business_id", "102", "wrong_business_identifier"),
        ("business_tenant", "tenant-b", "wrong_tenant"),
        ("job_tenant", "tenant-b", "wrong_job_tenant"),
        ("job_type", "system.user.import", "wrong_job_type"),
        ("job_id", "0", "invalid_job_identifier"),
    ] {
        let row = changed(job(), key, json!(value));
        assert_eq!(model::associated_job(&[row], &selection()), Err(reason));
    }
    assert_eq!(model::associated_job(&[], &selection()), Err("missing_job"));
    assert_eq!(
        model::associated_job(&[job(), job()], &selection()),
        Err("ambiguous_job")
    );
}

// Python: test_failed_cancelled_unfinished_and_missing_start_keep_evidence。
#[test]
fn failed_cancelled_unfinished_and_missing_start_keep_evidence() {
    for (row, attempts, reason) in [
        (
            changed(job(), "status", json!("running")),
            vec![attempt()],
            "job_unfinished",
        ),
        (
            changed(job(), "business_status", json!("cancelled")),
            vec![attempt()],
            "job_cancelled",
        ),
        (
            changed(job(), "status", json!("dead")),
            vec![attempt()],
            "job_failed",
        ),
        (
            job(),
            vec![changed(attempt(), "started_at", Value::Null)],
            "missing_attempt_timestamp",
        ),
        (
            job(),
            vec![changed(attempt(), "outcome", json!("lease_expired"))],
            "attempt_result_unknown",
        ),
    ] {
        assert_reason(&row, &attempts, reason);
    }
}

// Python: test_every_attempt_is_counted_without_retry_wait_as_execution。
#[test]
fn every_attempt_is_counted_without_retry_wait_as_execution() {
    let failed = changed(attempt(), "outcome", json!("failed"));
    let mut succeeded = attempt();
    succeeded["sequence"] = json!(2);
    succeeded["available_at"] = json!("2026-09-04T00:00:10Z");
    succeeded["started_at"] = json!("2026-09-04T00:00:12Z");
    succeeded["finished_at"] = json!("2026-09-04T00:00:15Z");
    succeeded["closed_at"] = json!("2026-09-04T00:00:15Z");
    let row = changed(job(), "claim_sequence", json!(2));
    let result = model::evidence(&row, &[failed, succeeded.clone()]);
    assert_eq!(result["valid"], true);
    assert_eq!(result["queue_ms"].as_f64(), Some(3000.0));
    assert_eq!(result["execution_ms"].as_f64(), Some(5000.0));
    assert_eq!(result["unsuccessful_attempts"], 1);
    assert_reason(&row, &[succeeded], "missing_attempt_history");
}

// Python: test_exact_message_and_schedule_links_do_not_guess_latest_job。
#[test]
fn exact_message_and_schedule_links_do_not_guess_latest_job() {
    let message_sql = sql::job_sql("devex_control", &message()).expect("消息查询");
    for part in [
        "o.aggregate_id=CAST(b.id AS CHAR)",
        "j.dedupe_key=o.dedupe_key",
        "o.event_type='system.message.published'",
        "'$.message_id'",
    ] {
        assert!(message_sql.contains(part));
    }
    let query = sql::job_sql("devex_control", &schedule()).expect("调度查询");
    assert!(query.contains("j.id=b.background_job_id AND j.schedule_id=b.schedule_id"));
    assert_eq!(
        model::associated_job(&[platform_job()], &schedule()),
        Ok(platform_job())
    );
    let skipped = changed(
        platform_job(),
        "business_status",
        json!("skipped_concurrency"),
    );
    assert_eq!(
        model::associated_job(&[skipped], &schedule()),
        Err("schedule_not_enqueued")
    );
}

// Python: test_input_injection_is_rejected_before_database_connection 的 selection 部分。
#[test]
fn input_injection_is_rejected_by_selection_and_sql() {
    for (key, value, reason) in [
        ("id", "1 OR 1=1", "invalid_job_identifier"),
        ("tenant_id", "x';--", "invalid_tenant"),
        ("id", "9223372036854775808", "invalid_job_identifier"),
        ("job_type", "other", "wrong_job_type"),
    ] {
        let expected = changed(selection(), key, json!(value));
        assert_eq!(model::selection(&expected), Err(reason));
        assert_eq!(sql::job_sql("devex_control", &expected), Err(reason));
    }
}

// Python: test_completion_waits_for_worker_even_after_business_cancel。
#[test]
fn completion_waits_for_worker_even_after_business_cancel() {
    assert!(
        model::completion(&[changed(job(), "status", json!("running"))], &selection()).is_none()
    );
    let mut row = changed(job(), "status", json!("pending"));
    row["business_status"] = json!("cancelled");
    assert!(model::completion(&[row], &selection()).is_none());
    let cancelled = changed(job(), "business_status", json!("cancelled"));
    assert_eq!(
        model::completion(&[cancelled], &selection()),
        Some(json!({
            "valid": false, "reason": "job_cancelled",
        }))
    );
    let dead = changed(job(), "status", json!("dead"));
    assert_eq!(
        model::completion(&[dead], &selection()),
        Some(json!({
            "valid": false, "reason": "job_failed",
        }))
    );
    assert_eq!(
        model::completion(&[job()], &selection()),
        Some(json!({
            "valid": true, "job_id": "201", "status": "succeeded",
        }))
    );
}

#[test]
fn identifiers_reject_booleans_numbers_and_out_of_range_strings() {
    for value in [json!("1"), json!("9223372036854775807")] {
        assert_eq!(model::identifier(&value), Ok(value.clone()));
    }
    for value in [
        json!(""),
        json!("0"),
        json!("01"),
        json!("-1"),
        json!("+1"),
        json!("1 "),
        json!("1\n"),
        json!("1e2"),
        json!("１"),
        json!("9223372036854775808"),
        json!("10000000000000000000"),
        json!(true),
        json!(false),
        json!(1),
        json!(1.0),
        Value::Null,
        json!([]),
        json!({}),
    ] {
        assert_eq!(model::identifier(&value), Err("invalid_job_identifier"));
    }
}

#[test]
fn selection_requires_exact_object_shape_and_known_kind() {
    for value in [
        Value::Null,
        json!(true),
        json!(1),
        json!("export"),
        json!([]),
        json!({}),
    ] {
        assert_eq!(model::selection(&value), Err("invalid_job_selection"));
    }
    let mut missing = selection();
    missing.as_object_mut().expect("对象").remove("id");
    assert_eq!(model::selection(&missing), Err("invalid_job_selection"));
    let extra = changed(selection(), "extra", json!(true));
    assert_eq!(model::selection(&extra), Err("invalid_job_selection"));
    for kind in [json!("other"), json!(true), json!({}), Value::Null] {
        assert_eq!(
            model::selection(&changed(selection(), "kind", kind)),
            Err("invalid_job_kind")
        );
    }
    for (kind, job_type) in [
        ("export", "system.export.execute"),
        ("import", "system.user.import"),
        ("message", "system.message.dispatch"),
        ("schedule", "custom.a-b_1"),
    ] {
        let mut expected = selection();
        expected["kind"] = json!(kind);
        expected["job_type"] = json!(job_type);
        assert_eq!(model::selection(&expected), Ok(expected.clone()));
    }
}

#[test]
fn selection_tenant_and_job_type_boundaries_are_strict() {
    for tenant in [json!("A0_-"), json!("a".repeat(64))] {
        assert!(model::selection(&changed(selection(), "tenant_id", tenant)).is_ok());
    }
    for tenant in [
        json!(""),
        json!("a".repeat(65)),
        json!("租户"),
        json!("a\n"),
        json!(true),
    ] {
        assert_eq!(
            model::selection(&changed(selection(), "tenant_id", tenant)),
            Err("invalid_tenant")
        );
    }
    let expected = changed(schedule(), "job_type", json!("a".repeat(96)));
    assert!(model::selection(&expected).is_ok());
    for job_type in [
        json!(""),
        json!("a".repeat(97)),
        json!("A"),
        json!("0a"),
        json!("a/b"),
        json!("a\n"),
        json!(true),
        json!([]),
    ] {
        assert_eq!(
            model::selection(&changed(schedule(), "job_type", job_type)),
            Err("wrong_job_type")
        );
    }
}

#[test]
fn association_guards_non_objects_and_limits_null_tenant_exception() {
    for row in [Value::Null, json!(true), json!(1), json!("row"), json!([])] {
        assert_eq!(
            model::associated_job(&[row], &selection()),
            Err("invalid_job_row")
        );
    }
    assert_eq!(
        model::associated_job(&[json!({})], &selection()),
        Err("invalid_job_identifier")
    );
    let no_tenant = changed(job(), "job_tenant", Value::Null);
    assert_eq!(
        model::associated_job(&[no_tenant], &selection()),
        Err("wrong_job_tenant")
    );
    let expected = changed(schedule(), "tenant_id", json!("tenant-a"));
    let row = changed(platform_job(), "business_tenant", json!("tenant-a"));
    assert_eq!(
        model::associated_job(&[row], &expected),
        Err("wrong_job_tenant")
    );
    let row = changed(platform_job(), "job_tenant", json!("other"));
    assert_eq!(
        model::associated_job(&[row], &schedule()),
        Err("wrong_job_tenant")
    );
    let row = changed(job(), "job_type", json!("system.message.dispatch"));
    assert_eq!(
        model::associated_job(std::slice::from_ref(&row), &message()),
        Err("message_not_dispatched")
    );
    let row = changed(row, "business_status", json!("published"));
    assert_eq!(
        model::associated_job(std::slice::from_ref(&row), &message()),
        Ok(row)
    );
    assert_eq!(
        model::associated_job(&[], &Value::Null),
        Err("invalid_job_selection")
    );
}

#[test]
fn every_business_terminal_failure_and_cancellation_keeps_reason() {
    for status in ["cancelled", "cancel_requested"] {
        let row = changed(job(), "business_status", json!(status));
        assert_reason(&row, &[attempt()], "job_cancelled");
        assert_eq!(
            model::completion(&[row], &selection()),
            Some(json!({
                "valid": false, "reason": "job_cancelled",
            }))
        );
    }
    for status in ["failed", "partial", "dead", "expired"] {
        let row = changed(job(), "business_status", json!(status));
        assert_reason(&row, &[attempt()], "job_failed");
        assert_eq!(
            model::completion(&[row], &selection()),
            Some(json!({
                "valid": false, "reason": "job_failed",
            }))
        );
    }
    for status in [Value::Null, json!(true), json!("pending"), json!("unknown")] {
        let row = changed(job(), "business_status", status);
        assert_reason(&row, &[attempt()], "job_unfinished");
        assert!(model::completion(&[row], &selection()).is_none());
    }
    for status in ["succeeded", "published", "enqueued"] {
        let row = changed(job(), "business_status", json!(status));
        assert_eq!(model::evidence(&row, &[attempt()])["valid"], true);
    }
}

#[test]
fn attempt_history_requires_positive_integer_count_and_exact_identity() {
    for count in [
        Value::Null,
        json!(true),
        json!(false),
        json!(0),
        json!(-1),
        json!(1.0),
        json!("1"),
        json!(2),
        json!(u64::MAX),
    ] {
        assert_reason(
            &changed(job(), "claim_sequence", count),
            &[attempt()],
            "missing_attempt_history",
        );
    }
    assert_reason(&job(), &[], "missing_attempt_history");
    for sequence in [
        Value::Null,
        json!(true),
        json!(false),
        json!(0),
        json!(2),
        json!("1"),
        json!(1.0),
    ] {
        assert_reason(
            &job(),
            &[changed(attempt(), "sequence", sequence)],
            "wrong_attempt_identity",
        );
    }
    for job_id in [Value::Null, json!(true), json!(201), json!("202")] {
        assert_reason(
            &job(),
            &[changed(attempt(), "job_id", job_id)],
            "wrong_attempt_identity",
        );
    }
    for value in [
        Value::Null,
        json!(false),
        json!(1),
        json!([]),
        json!("attempt"),
    ] {
        assert_reason(
            &job(),
            std::slice::from_ref(&value),
            "wrong_attempt_identity",
        );
        assert_reason(&value, &[attempt()], "invalid_job_row");
        assert_eq!(
            model::attempt_times(&value, &[attempt()]),
            Err("invalid_job_row")
        );
    }
}

#[test]
fn all_attempt_outcomes_are_preserved() {
    for (outcome, reason) in [
        (json!("running"), "unfinished_attempt"),
        (json!("lease_expired"), "attempt_result_unknown"),
        (json!("failed"), "last_attempt_not_successful"),
        (json!("dead"), "last_attempt_not_successful"),
        (json!("deferred"), "last_attempt_not_successful"),
        (json!("unknown"), "invalid_attempt_outcome"),
        (json!(true), "invalid_attempt_outcome"),
        (json!({}), "invalid_attempt_outcome"),
        (Value::Null, "invalid_attempt_outcome"),
    ] {
        assert_reason(&job(), &[changed(attempt(), "outcome", outcome)], reason);
    }
    let row = changed(job(), "claim_sequence", json!(2));
    for outcome in ["failed", "dead", "deferred", "succeeded"] {
        let first = changed(attempt(), "outcome", json!(outcome));
        let mut second = changed(attempt(), "sequence", json!(2));
        second["available_at"] = json!("2026-09-04T00:00:03Z");
        second["started_at"] = json!("2026-09-04T00:00:03Z");
        let result = model::evidence(&row, &[first, second]);
        assert_eq!(result["valid"], true);
        assert_eq!(
            result["unsuccessful_attempts"],
            u64::from(outcome != "succeeded")
        );
    }
}

#[test]
fn timestamps_require_valid_timezone_and_retain_microseconds() {
    for value in [Value::Null, json!(true), json!(1), json!([]), json!({})] {
        assert_eq!(model::timestamp(&value), Err("missing_attempt_timestamp"));
    }
    for value in [
        "invalid secret",
        "2026-02-30T00:00:00Z",
        "2026-09-04T00:00:60Z",
        "0000-01-01T00:00:00Z",
        "0000-01-01T00:00:00",
        "0000-01-01",
        "2026-09-04T00:00:00z",
        "",
    ] {
        assert_eq!(
            model::timestamp(&json!(value)),
            Err("invalid_attempt_timestamp")
        );
    }
    for value in [
        "2026-09-04T00:00:00.000001",
        "2026-09-04 00:00:00",
        "2026-09-04",
    ] {
        assert_eq!(
            model::timestamp(&json!(value)),
            Err("timestamp_without_timezone")
        );
    }
    for (value, expected) in [
        (
            "2026-09-04T00:00:00.000001Z",
            "2026-09-04T00:00:00.000001+00:00",
        ),
        (
            "2026-09-04T08:00:00.123456+08:00",
            "2026-09-04T08:00:00.123456+08:00",
        ),
        (
            "2026-09-03T18:30:00.123456-05:30",
            "2026-09-03T18:30:00.123456-05:30",
        ),
        (
            "2026-09-04T00:00:00.123456789Z",
            "2026-09-04T00:00:00.123456+00:00",
        ),
        (
            "2026-09-04T00:00:00.123456789012345Z",
            "2026-09-04T00:00:00.123456+00:00",
        ),
        (
            "2026-09-04T08:00:00,1234567+08:00",
            "2026-09-04T08:00:00.123456+08:00",
        ),
        (
            "2026-09-04 08:00:00+0800",
            "2026-09-04T08:00:00.000000+08:00",
        ),
    ] {
        let parsed = model::timestamp(&json!(value))
            .map(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::Micros, false));
        assert_eq!(parsed, Ok(expected.to_owned()));
    }
}

#[test]
fn fractional_milliseconds_and_offset_order_are_not_truncated() {
    let mut first = attempt();
    first["outcome"] = json!("failed");
    first["available_at"] = json!("2026-09-04T00:00:00.000001Z");
    first["started_at"] = json!("2026-09-04T08:00:00.000003+08:00");
    first["finished_at"] = json!("2026-09-03T18:30:00.000010-05:30");
    first["closed_at"] = json!("2026-09-04T00:00:00.000010Z");
    let mut second = changed(attempt(), "sequence", json!(2));
    second["available_at"] = json!("2026-09-04T00:00:10.000001Z");
    second["started_at"] = json!("2026-09-04T00:00:10.000004Z");
    second["finished_at"] = json!("2026-09-04T00:00:10.000015Z");
    second["closed_at"] = second["finished_at"].clone();
    let result = model::evidence(
        &changed(job(), "claim_sequence", json!(2)),
        &[first, second],
    );
    assert_eq!(result["valid"], true);
    assert!((result["queue_ms"].as_f64().expect("浮点毫秒") - 0.005).abs() < 1e-12);
    assert!((result["execution_ms"].as_f64().expect("浮点毫秒") - 0.018).abs() < 1e-12);
}

#[test]
fn timestamp_failures_and_order_are_checked_for_every_field() {
    for key in ["available_at", "started_at", "finished_at", "closed_at"] {
        for (value, reason) in [
            (Value::Null, "missing_attempt_timestamp"),
            (json!("invalid secret"), "invalid_attempt_timestamp"),
            (json!("2026-09-04T00:00:00"), "timestamp_without_timezone"),
        ] {
            assert_reason(&job(), &[changed(attempt(), key, value)], reason);
        }
    }
    for (key, value) in [
        ("available_at", "2026-09-04T00:00:02Z"),
        ("started_at", "2026-09-04T00:00:04Z"),
        ("closed_at", "2026-09-04T00:00:03.000001Z"),
    ] {
        assert_reason(
            &job(),
            &[changed(attempt(), key, json!(value))],
            "unordered_attempt_timestamps",
        );
    }
    let first = changed(attempt(), "outcome", json!("failed"));
    let second = changed(attempt(), "sequence", json!(2));
    assert_reason(
        &changed(job(), "claim_sequence", json!(2)),
        &[first, second],
        "unordered_attempt_timestamps",
    );
}

#[test]
fn completion_distinguishes_pending_association_and_terminal_errors() {
    assert!(model::completion(&[], &selection()).is_none());
    let unpublished = changed(job(), "job_type", json!("system.message.dispatch"));
    assert!(model::completion(&[unpublished], &message()).is_none());
    assert_eq!(
        model::completion(&[job(), job()], &selection()),
        Some(json!({
            "valid": false, "reason": "ambiguous_job",
        }))
    );
    assert_eq!(
        model::completion(&[Value::Null], &selection()),
        Some(json!({
            "valid": false, "reason": "invalid_job_row",
        }))
    );
    assert_eq!(
        model::completion(&[], &Value::Null),
        Some(json!({
            "valid": false, "reason": "invalid_job_selection",
        }))
    );
    let skipped = changed(
        platform_job(),
        "business_status",
        json!("skipped_concurrency"),
    );
    assert_eq!(
        model::completion(&[skipped], &schedule()),
        Some(json!({
            "valid": false, "reason": "schedule_not_enqueued",
        }))
    );
    assert_eq!(
        model::completion(&[platform_job()], &schedule()),
        Some(json!({
            "valid": true, "job_id": "201", "status": "succeeded",
        }))
    );
}

#[test]
fn completion_rechecks_each_supplied_snapshot() {
    let mut row = job();
    for status in ["pending", "running", "succeeded"] {
        row["status"] = json!(status);
        let result = model::completion(&[row.clone()], &selection());
        if status == "succeeded" {
            assert_eq!(
                result,
                Some(json!({
                    "valid": true, "job_id": "201", "status": "succeeded",
                }))
            );
        } else {
            assert!(result.is_none());
        }
    }
    row["business_tenant"] = json!("tenant-b");
    assert_eq!(
        model::completion(&[row], &selection()),
        Some(json!({
            "valid": false, "reason": "wrong_tenant",
        }))
    );
    let mut row = changed(job(), "job_type", json!("system.message.dispatch"));
    assert!(model::completion(&[row.clone()], &message()).is_none());
    row["business_status"] = json!("published");
    assert_eq!(
        model::completion(&[row], &message()),
        Some(json!({
            "valid": true, "job_id": "201", "status": "succeeded",
        }))
    );
}

#[test]
fn terminal_failure_precedence_and_worker_wait_match_python() {
    for business_status in [
        "cancelled",
        "cancel_requested",
        "failed",
        "partial",
        "dead",
        "expired",
    ] {
        let mut row = changed(job(), "business_status", json!(business_status));
        for status in ["pending", "running", "unknown"] {
            row["status"] = json!(status);
            assert!(model::completion(&[row.clone()], &selection()).is_none());
        }
        row["status"] = json!("dead");
        let reason = if business_status.starts_with("cancel") {
            "job_cancelled"
        } else {
            "job_failed"
        };
        assert_reason(&row, &[], reason);
        assert_eq!(
            model::completion(&[row], &selection()),
            Some(json!({
                "valid": false, "reason": reason,
            }))
        );
    }
    for outcome in ["running", "lease_expired", "unknown"] {
        let mut row = changed(attempt(), "outcome", json!(outcome));
        row["finished_at"] = Value::Null;
        let reason = match outcome {
            "running" => "unfinished_attempt",
            "lease_expired" => "attempt_result_unknown",
            _ => "invalid_attempt_outcome",
        };
        assert_reason(&job(), &[row], reason);
    }
}

#[test]
fn entire_attempt_history_is_retained_and_retry_overlap_uses_start_order() {
    let row = changed(job(), "claim_sequence", json!(4));
    let attempts = ["failed", "dead", "deferred", "succeeded"]
        .into_iter()
        .enumerate()
        .map(|(index, outcome)| {
            let second = index * 3;
            let mut value = changed(attempt(), "sequence", json!(index + 1));
            value["outcome"] = json!(outcome);
            value["available_at"] = json!(format!("2026-09-04T00:00:{second:02}Z"));
            value["started_at"] = json!(format!("2026-09-04T00:00:{:02}Z", second + 1));
            value["finished_at"] = json!(format!("2026-09-04T00:00:{:02}Z", second + 3));
            value["closed_at"] = value["finished_at"].clone();
            value
        })
        .collect::<Vec<_>>();
    let result = model::evidence(&row, &attempts);
    assert_eq!(result["valid"], true);
    assert_eq!(result["attempt_count"], 4);
    assert_eq!(result["unsuccessful_attempts"], 3);
    assert_eq!(result["queue_ms"].as_f64(), Some(4000.0));
    assert_eq!(result["execution_ms"].as_f64(), Some(8000.0));
    assert_eq!(result["attempts"], json!(attempts));
    for index in 0..attempts.len() {
        let mut invalid = attempts.clone();
        invalid[index]["sequence"] = json!(index + 2);
        assert_reason(&row, &invalid, "wrong_attempt_identity");
    }
    let mut second = changed(attempt(), "sequence", json!(2));
    second["started_at"] = json!("2026-09-04T00:00:03Z");
    // 后一次入队时间可以早于前一次完成，原模型只要求领取时间不重叠。
    let result = model::evidence(
        &changed(job(), "claim_sequence", json!(2)),
        &[changed(attempt(), "outcome", json!("failed")), second],
    );
    assert_eq!(result["valid"], true);
    assert_eq!(result["queue_ms"].as_f64(), Some(4000.0));
    assert_eq!(result["execution_ms"].as_f64(), Some(2000.0));
}

#[test]
fn sql_has_exact_fixed_export_and_attempt_statements() {
    let export = concat!(
        "SELECT JSON_OBJECT('business_id',CAST(b.id AS CHAR),'business_tenant',b.tenant_id,",
        "'business_status',b.status,'job_id',CAST(j.id AS CHAR),'job_tenant',j.tenant_id,",
        "'job_type',j.job_type,'status',j.status,'claim_sequence',j.claim_sequence) ",
        "FROM `devex_control`.`sys_export_job` b JOIN `devex_control`.sys_background_job j ",
        "ON j.id=b.background_job_id WHERE b.id=101 AND b.tenant_id='tenant-a' LIMIT 2",
    );
    assert_eq!(
        sql::job_sql("devex_control", &selection()),
        Ok(export.to_owned())
    );
    let attempts = concat!(
        "SELECT JSON_OBJECT('job_id',CAST(job_id AS CHAR),'sequence',`sequence`,'outcome',outcome,",
        "'available_at',DATE_FORMAT(`available_at`,'%Y-%m-%dT%H:%i:%s.%fZ'),",
        "'started_at',DATE_FORMAT(`started_at`,'%Y-%m-%dT%H:%i:%s.%fZ'),",
        "'finished_at',DATE_FORMAT(`finished_at`,'%Y-%m-%dT%H:%i:%s.%fZ'),",
        "'closed_at',DATE_FORMAT(`closed_at`,'%Y-%m-%dT%H:%i:%s.%fZ')) ",
        "FROM `devex_control`.sys_background_job_attempt WHERE job_id=201 ORDER BY `sequence` LIMIT 10001",
    );
    assert_eq!(
        sql::attempts_sql("devex_control", "201"),
        Ok(attempts.to_owned())
    );
}

#[test]
fn sql_keeps_import_cancellation_and_all_exact_join_predicates() {
    let expected = changed(
        changed(selection(), "kind", json!("import")),
        "job_type",
        json!("system.user.import"),
    );
    let query = sql::job_sql("devex_control", &expected).expect("导入 SQL");
    assert!(query.contains("`devex_control`.`sys_user_import_job` b JOIN"));
    assert!(query.contains("'business_status',IF(b.cancel_requested,'cancel_requested',b.status)"));
    let message = sql::job_sql("devex_control", &message()).expect("消息 SQL");
    for part in [
        "o.aggregate_type='message'",
        "o.tenant_id=b.tenant_id",
        "j.dedupe_key=CONCAT('message:',b.id)",
        "JSON_UNQUOTE(JSON_EXTRACT(j.payload,'$.message_id'))=CAST(b.id AS CHAR)",
        "'business_status',o.status",
    ] {
        assert!(message.contains(part));
    }
    let schedule = sql::job_sql("devex_control", &schedule()).expect("调度 SQL");
    assert!(schedule.contains("'business_status',b.outcome"));
    assert!(schedule.contains(
        "JSON_UNQUOTE(JSON_EXTRACT(j.payload,'$.schedule_id'))=CAST(b.schedule_id AS CHAR)"
    ));
}

#[test]
fn sql_has_exact_import_message_and_schedule_statements() {
    let prefix = concat!(
        "SELECT JSON_OBJECT('business_id',CAST(b.id AS CHAR),'business_tenant',b.tenant_id,",
        "'business_status',",
    );
    let columns = concat!(
        ",'job_id',CAST(j.id AS CHAR),'job_tenant',j.tenant_id,",
        "'job_type',j.job_type,'status',j.status,'claim_sequence',j.claim_sequence) FROM ",
    );
    let import = changed(
        changed(selection(), "kind", json!("import")),
        "job_type",
        json!("system.user.import"),
    );
    for (expected, state, source) in [
        (
            import,
            "IF(b.cancel_requested,'cancel_requested',b.status)",
            concat!(
                "`devex_control`.`sys_user_import_job` b JOIN `devex_control`.sys_background_job j ",
                "ON j.id=b.background_job_id",
            ),
        ),
        (
            message(),
            "o.status",
            concat!(
                "`devex_control`.sys_message b JOIN `devex_control`.sys_outbox_event o ",
                "ON o.aggregate_type='message' AND o.aggregate_id=CAST(b.id AS CHAR) ",
                "AND o.tenant_id=b.tenant_id AND o.event_type='system.message.published' ",
                "JOIN `devex_control`.sys_background_job j ON j.dedupe_key=o.dedupe_key ",
                "AND j.dedupe_key=CONCAT('message:',b.id) ",
                "AND JSON_UNQUOTE(JSON_EXTRACT(j.payload,'$.message_id'))=CAST(b.id AS CHAR)",
            ),
        ),
        (
            schedule(),
            "b.outcome",
            concat!(
                "`devex_control`.sys_job_schedule_execution b JOIN `devex_control`.sys_background_job j ",
                "ON j.id=b.background_job_id AND j.schedule_id=b.schedule_id ",
                "AND JSON_UNQUOTE(JSON_EXTRACT(j.payload,'$.schedule_id'))=CAST(b.schedule_id AS CHAR)",
            ),
        ),
    ] {
        let tenant = expected["tenant_id"].as_str().expect("租户标识");
        let query = format!(
            "{prefix}{state}{columns}{source} WHERE b.id=101 AND b.tenant_id='{tenant}' LIMIT 2"
        );
        assert_eq!(sql::job_sql("devex_control", &expected), Ok(query));
    }
}

#[test]
fn sql_rejects_schema_and_id_injection_with_fixed_reasons() {
    for database in [
        "",
        "db`",
        "db-name",
        "db.name",
        "db;DROP TABLE x",
        "数据库",
        "db\n",
        &"a".repeat(65),
    ] {
        assert_eq!(
            sql::job_sql(database, &selection()),
            Err("invalid_database_identifier")
        );
        assert_eq!(
            sql::attempts_sql(database, "201"),
            Err("invalid_database_identifier")
        );
    }
    for database in ["A0_", "0", &"a".repeat(64)] {
        assert!(sql::job_sql(database, &selection()).is_ok());
        assert!(sql::attempts_sql(database, "201").is_ok());
    }
    for id in ["0", "01", "1 OR 1=1", "9223372036854775808", "true", "1\n"] {
        assert_eq!(
            sql::attempts_sql("devex_control", id),
            Err("invalid_job_identifier")
        );
    }
    assert!(sql::attempts_sql("devex_control", "9223372036854775807").is_ok());
}
