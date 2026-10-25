use super::{close, rebind_receipt, result, transition};
use serde_json::{Value, json};
use std::path::Path;

fn snapshot() -> Value {
    json!({"database_time":"2026-09-05T00:00:00.000000Z",
        "active":close::ACTIVE.iter().map(|key| (key.to_string(),json!(0))).collect::<serde_json::Map<_,_>>(),
        "terminal_non_success":close::TERMINAL.iter().map(|key| (key.to_string(),json!(0))).collect::<serde_json::Map<_,_>>()})
}

#[test]
fn snapshot_requires_exact_counts_time_and_true_integers() {
    let original = snapshot();
    assert!(close::drained(&original).unwrap());
    for field in ["active", "terminal_non_success"] {
        for changed in [json!(true), json!(-1), json!(1.5), json!("0"), Value::Null] {
            let mut value = original.clone();
            let key = value[field]
                .as_object()
                .unwrap()
                .keys()
                .next()
                .unwrap()
                .clone();
            value[field][key] = changed;
            assert!(close::snapshot(&value).is_err());
        }
        let mut value = original.clone();
        value[field]
            .as_object_mut()
            .unwrap()
            .remove(if field == "active" {
                close::ACTIVE[0]
            } else {
                close::TERMINAL[0]
            });
        assert!(close::snapshot(&value).is_err());
        value[field]["unexpected"] = json!(0);
        assert!(close::snapshot(&value).is_err());
    }
    for time in [
        "local-time",
        "2026-09-05T00:00:00Z",
        "2026-09-05T00:00:00.000000Z\n",
        "２０２６-09-05T00:00:00.000000Z",
    ] {
        let mut value = original.clone();
        value["database_time"] = json!(time);
        assert!(close::snapshot(&value).is_err());
    }
}

#[test]
fn terminal_failures_are_visible_but_do_not_mean_active_work() {
    let mut value = snapshot();
    value["terminal_non_success"]["outbox_event_dead"] = json!(2);
    assert!(close::drained(&value).unwrap());
    value["active"]["outbox_event_status"] = json!(1);
    assert!(!close::drained(&value).unwrap());
}

#[test]
fn stable_drain_checks_both_images_and_worker_creation_identity() {
    let (_, _, worker) = result::fixture();
    let original = json!({"stable_seconds":2,"observations":3,"first_snapshot":snapshot(),"last_snapshot":snapshot()});
    assert!(result::drain(&original, &worker).is_ok());
    for (field, changed) in [
        ("stable_seconds", json!(true)),
        ("stable_seconds", json!(1)),
        ("observations", json!(true)),
        ("observations", json!(1)),
        ("observations", json!(2.0)),
    ] {
        let mut value = original.clone();
        value[field] = changed;
        assert!(result::drain(&value, &worker).is_err());
    }
    for field in ["active", "terminal_non_success"] {
        let mut value = original.clone();
        value["last_snapshot"][field][if field == "active" {
            close::ACTIVE[0]
        } else {
            close::TERMINAL[0]
        }] = json!(1);
        assert!(result::drain(&value, &worker).is_err());
    }
    let mut value = original.clone();
    value["last_snapshot"]["database_time"] = json!("2026-09-05T00:00:02.000000Z");
    assert!(result::drain(&value, &worker).is_ok());
    assert!(result::drain(&original, &json!({"pid":1,"started":"12","executable":"a"})).is_err());
}

#[test]
fn control_rejects_unknown_fields_roles_non_boolean_readiness_and_reused_pid() {
    let (value, handoff, identity) = result::fixture();
    let control = &value["runtime_status"];
    let runtime = Path::new("fixture");
    assert!(close::require(control, "initial", runtime, &handoff, &Value::Null).is_ok());
    assert!(
        close::require(
            control,
            "fixed",
            runtime,
            &handoff,
            &value["process_identities"]
        )
        .is_ok()
    );
    for (field, changed) in [
        ("format_version", json!(true)),
        ("operation", json!("stop")),
        ("scope_id", json!("other")),
        ("runtime_directory", json!("other")),
        ("producer_history", json!("other")),
    ] {
        let mut changed_control = control.clone();
        changed_control[field] = changed;
        assert!(
            close::require(&changed_control, "initial", runtime, &handoff, &Value::Null).is_err()
        );
    }
    for changed in [json!(1), json!(false), Value::Null] {
        let mut changed_control = control.clone();
        changed_control["processes"]["worker"]["ready"] = changed;
        assert!(
            close::require(&changed_control, "initial", runtime, &handoff, &Value::Null).is_err()
        );
    }
    let mut changed_control = control.clone();
    changed_control["processes"]["worker"]["identity"]["started"] = json!("124");
    assert!(
        close::require(
            &changed_control,
            "fixed",
            runtime,
            &handoff,
            &value["process_identities"]
        )
        .is_err()
    );
    assert!(close::identity(&identity).is_ok());
}

#[test]
fn only_exact_stopped_state_is_allowed_in_closed_or_drain_observation() {
    let (value, handoff, _) = result::fixture();
    let mut control = value["runtime_status"].clone();
    let runtime = Path::new("fixture");
    control["processes"]["api"] = close::stopped();
    assert!(
        close::require(
            &control,
            "draining",
            runtime,
            &handoff,
            &value["process_identities"]
        )
        .is_ok()
    );
    assert!(close::require(&control, "closed", runtime, &handoff, &Value::Null).is_err());
    control["processes"]["worker"] = close::stopped();
    assert!(close::require(&control, "closed", runtime, &handoff, &Value::Null).is_ok());
    assert!(
        close::require(
            &control,
            "observed",
            runtime,
            &handoff,
            &value["process_identities"]
        )
        .is_ok()
    );
    control["processes"]["worker"]["identity"] = value["process_identities"]["worker"].clone();
    assert!(close::require(&control, "closed", runtime, &handoff, &Value::Null).is_err());
}

#[test]
fn close_result_binds_handoff_types_and_reconciliation_reason() {
    let (original, handoff, _) = result::fixture();
    let runtime = Path::new("fixture");
    assert!(result::validate(&original, runtime, &handoff, &json!({}), true).is_ok());
    assert!(result::validate(&original, runtime, &handoff, &json!({}), false).is_err());
    let mut value = original.clone();
    value["reason"] = json!("api_stop_failed");
    assert!(result::validate(&value, runtime, &handoff, &json!({}), false).is_ok());
    for (field, changed) in [
        ("handoff", json!({"different":true})),
        ("direct_remote_writes", json!(true)),
        ("worker_drain_writes", json!(1)),
        ("restore_qualified", json!(0)),
        ("reason", json!("unknown")),
        ("error_type", json!("secret password")),
        ("worker_preserved", json!(false)),
    ] {
        let mut changed_value = value.clone();
        changed_value[field] = changed;
        assert!(result::validate(&changed_value, runtime, &handoff, &json!({}), false).is_err());
    }
}

#[test]
fn close_success_requires_verified_complete_drained_evidence() {
    let (mut value, handoff, identity) = result::fixture();
    let runtime = Path::new("fixture");
    let control = value["runtime_status"].clone();
    let mut api_stop = control.clone();
    api_stop["operation"] = json!("stop");
    api_stop["processes"] = json!({"api":close::stopped()});
    value["api_stop"] = api_stop;
    let mut worker_stop = control;
    worker_stop["operation"] = json!("stop");
    worker_stop["processes"] = json!({"worker":close::stopped()});
    value["worker_stop"] = worker_stop;
    value["runtime_status"]["processes"] =
        json!({"api":close::stopped(),"worker":close::stopped()});
    value["drain"] = json!({"stable_seconds":2,"observations":3,"first_snapshot":snapshot(),"last_snapshot":snapshot()});
    value["status"] = json!("seed_runtime_closed");
    value["phase"] = json!("verified");
    value["worker_preserved"] = json!(false);
    value["outbox_drained"] = json!(true);
    value["final_snapshot"] = snapshot();
    value["terminal_non_success"] = value["final_snapshot"]["terminal_non_success"].clone();
    value["stopped_evidence"] = json!({"original":identity});
    assert!(result::validate(&value, runtime, &handoff, &json!({}), false).is_ok());
    for field in ["drain", "final_snapshot", "stopped_evidence"] {
        let mut changed = value.clone();
        changed[field] = Value::Null;
        assert!(result::validate(&changed, runtime, &handoff, &json!({}), false).is_err());
    }
    let mut changed = value.clone();
    changed["final_snapshot"]["active"][close::ACTIVE[0]] = json!(1);
    assert!(result::validate(&changed, runtime, &handoff, &json!({}), false).is_err());
}

fn storage() -> Value {
    json!({"attempt":7,"request":{"sha256":"fixed"},"data_directory":{"path":"data"},
        "api_url":"http://127.0.0.1:29000","console_url":"http://127.0.0.1:29001",
        "storage":{"sha256":"binary","identity":{"pid":101,"started":"100","executable":"rustfs.exe"}}})
}

#[test]
fn rebind_transition_preserves_request_directory_endpoints_and_binary() {
    let original = storage();
    let mut current = original.clone();
    current["attempt"] = json!(53);
    current["storage"]["identity"]["pid"] = json!(102);
    assert!(transition(&original, &current).is_ok());
    for bad in [Value::Null, original.clone()] {
        assert!(transition(&original, &bad).is_err());
    }
    for field in [
        "request",
        "data_directory",
        "api_url",
        "console_url",
        "attempt",
    ] {
        let mut bad = current.clone();
        bad[field] = json!(6);
        assert!(transition(&original, &bad).is_err());
    }
    for field in ["sha256", "identity"] {
        let mut bad = current.clone();
        bad["storage"][field] = json!("changed");
        assert!(transition(&original, &bad).is_err());
    }
    for attempt in [json!(true), json!(53.0), json!(-1), json!(7)] {
        let mut bad = current.clone();
        bad["attempt"] = attempt;
        assert!(transition(&original, &bad).is_err());
    }
}

#[test]
fn rebind_receipt_binds_full_prefix_and_zero_write_semantics() {
    let prefix = json!([{"number":52,"status":"passed"}]);
    let receipt = json!({"status":"seed_source_rebound","source_registration":{"sha256":"source"},
        "review_successor":{},"original_storage":storage(),"current_storage":{},"history_length":1,
        "history_sha256":crate::recovery::clone::native::digest::plan_hash(&prefix).unwrap(),"remote_writes":0,"restore_qualified":false});
    let input = json!({"value":receipt,"descriptor":{"sha256":"source"},"original":storage(),"prefix":prefix});
    assert!(rebind_receipt(&input).is_ok());
    for (field, changed) in [
        ("history_length", json!(true)),
        ("history_length", json!(0)),
        ("history_sha256", json!("other")),
        ("remote_writes", json!(true)),
        ("restore_qualified", json!(true)),
        ("source_registration", json!({})),
        ("original_storage", Value::Null),
        ("status", json!("incomplete")),
    ] {
        let mut bad = input.clone();
        bad["value"][field] = changed;
        assert!(rebind_receipt(&bad).is_err());
    }
    let mut bad = input.clone();
    bad["prefix"][0]["status"] = json!("failed");
    assert!(rebind_receipt(&bad).is_err());
}

#[test]
fn control_request_rejects_unknown_or_repeated_role_subsets() {
    let (value, handoff, _) = result::fixture();
    let request = json!({"value":value["runtime_status"], "requirement":"validate", "operation":"status",
        "roles":["api","worker"], "runtime":"fixture", "handoff":handoff, "identities":null});
    assert!(close::control_request(&request).is_ok());
    for roles in [
        json!([]),
        json!(["api", "api"]),
        json!(["api", "worker", "api"]),
        json!(["web"]),
    ] {
        let mut bad = request.clone();
        bad["roles"] = roles;
        assert!(close::control_request(&bad).is_err());
    }
}
