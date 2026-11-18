use super::*;
use successor::{
    check_close, check_first, check_first_runtime_path, check_redis_successor, close_row, restarts,
};

#[test]
fn active_fixture_successor_must_be_first_or_second_accepted_restart() {
    let first = json!({"stage":"fixture-services","mode":"restart","status":"passed",
        "result":{"path":"original-restart"}});
    let second = json!({"stage":"fixture-services","mode":"restart","status":"passed",
        "result":{"path":"second-restart"}});
    let failed = json!({"stage":"fixture-services","mode":"restart","status":"failed"});
    let mut history = json!({"active_generation":first["result"],"closed":false,
        "external_recovery":null,"rustfs_handoff":null});
    assert_eq!(
        restarts(&history, &[failed, first.clone()]).unwrap(),
        (&first, None)
    );
    assert!(restarts(&history, &[first.clone(), second.clone()]).is_err());
    history["active_generation"] = second["result"].clone();
    assert_eq!(
        restarts(&history, &[first.clone(), second.clone()]).unwrap(),
        (&first, Some(&second))
    );
    assert!(restarts(&history, &[first.clone(), second.clone(), second.clone()]).is_err());
    history["external_recovery"] = json!({"path":"newer-external-termination"});
    assert!(restarts(&history, &[first, second]).is_err());
}

#[test]
fn windows_successor_accepts_exact_original_endpoint_and_requires_bound_external_termination() {
    let original = json!({"platform":"windows","port":23700,"configuration":{"sha256":"config"},
        "tool":{"sha256":"registered"},"pid":100,"started":"200","run_id":"original"});
    let mut selected = original.clone();
    selected["pid"] = json!(101);
    selected["started"] = json!("201");
    selected["run_id"] = json!("successor");
    let previous = json!({"format_version":3,"process":{"pid":100},"redis":original});
    let request = json!({"format_version":3,"previous_identity":previous["process"],"previous_run_id":"original"});
    let generation =
        json!({"previous_generation":{"kind":"initial"},"redis":{"runtime":{"redis":selected}}});
    let proof = json!({"format_version":2,"kind":"reference-fixture-external-termination",
        "observation":{"evidence":{"redis_external":{"kind":"windows-redis-external-termination"}}}});
    check_first(
        &original,
        &selected,
        &previous,
        &request,
        &generation,
        &proof,
    )
    .unwrap();
    for (key, drift) in [
        ("port", json!(23701)),
        ("run_id", json!("original")),
        ("tool", json!({"sha256":"other"})),
    ] {
        let mut invalid = selected.clone();
        invalid[key] = drift;
        assert!(
            check_first(
                &original,
                &invalid,
                &previous,
                &request,
                &generation,
                &proof
            )
            .is_err()
        );
    }
    let mut invalid = proof.clone();
    invalid["kind"] = json!("reference-fixture-normal-close");
    assert!(
        check_first(
            &original,
            &selected,
            &previous,
            &request,
            &generation,
            &invalid
        )
        .is_err()
    );
}

#[test]
fn second_restart_binds_the_first_generation_and_its_nonzero_exit_reconciliation() {
    let first_result = json!({"path":"first-restart"});
    let original = json!({"platform":"windows","port":23700,"configuration":{"sha256":"config"},
        "tool":{"sha256":"registered"},"pid":100,"started":"200","run_id":"initial"});
    let mut first_redis = original.clone();
    first_redis["pid"] = json!(101);
    first_redis["started"] = json!("201");
    first_redis["run_id"] = json!("first");
    let first_runtime = json!({"format_version":3,"process":{"pid":101},"redis":first_redis});
    let first = json!({"redis":{"runtime":first_runtime,
        "request":{"path":"first-request","bytes":123,"sha256":"a".repeat(64)}}});
    let mut second_redis = original.clone();
    second_redis["pid"] = json!(102);
    second_redis["started"] = json!("202");
    second_redis["run_id"] = json!("second");
    let second = json!({"previous_generation":first_result});
    let request = json!({"format_version":3,"previous_identity":first_runtime["process"],
        "previous_run_id":"first"});
    let outer = json!({"status":"failed_close_reconciled","failed_attempt":10});
    let reconciliation = json!({"kind":"reference-fixture-failed-close-reconciliation"});
    let recovery = json!({"format_version":2,"kind":"reference-fixture-close-recovery",
        "candidate":{"failed_attempt":10,"request":first["redis"]["request"],
        "prior_windows_redis_exit":{"returncode":-1}}});
    let validate = |proof: &Value, runtime: &Value, generation: &Value| {
        check_close(
            &outer,
            &reconciliation,
            proof,
            runtime,
            (&first_result, &first),
            generation,
        )
    };
    validate(&recovery, &first_runtime, &second).unwrap();
    check_redis_successor(&original, &second_redis, &first_runtime, &request).unwrap();
    for (pointer, value) in [
        ("/format_version", json!(1)),
        ("/candidate/prior_windows_redis_exit/returncode", json!(0)),
        ("/candidate/failed_attempt", json!(9)),
        ("/candidate/request/path", json!("different-request")),
    ] {
        let mut invalid = recovery.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        assert!(validate(&invalid, &first_runtime, &second).is_err());
    }
    assert!(
        validate(
            &recovery,
            &first_runtime,
            &json!({"previous_generation":{"kind":"initial"}})
        )
        .is_err()
    );
    let mut wrong_runtime = first_runtime.clone();
    wrong_runtime["process"]["pid"] = json!(200);
    assert!(validate(&recovery, &wrong_runtime, &second).is_err());
    for field in ["started", "run_id"] {
        let mut wrong_selected = second_redis.clone();
        wrong_selected[field] = first_redis[field].clone();
        assert!(
            check_redis_successor(&original, &wrong_selected, &first_runtime, &request).is_err()
        );
    }
    let mut wrong_request = request;
    wrong_request["previous_run_id"] = json!("initial");
    assert!(
        check_redis_successor(&original, &second_redis, &first_runtime, &wrong_request).is_err()
    );
}

#[test]
fn second_restart_requires_first_runtime_binding_path() {
    let run = std::path::Path::new("fixture");
    let output = run.join("lifecycle-0008").join("redis");
    let runtime = json!({"output":output});
    let binding = json!({"path":output.join("runtime.json"),"bytes":21,"sha256":"a".repeat(64)});
    check_first_runtime_path(run, 8, &runtime, &binding).unwrap();
    assert!(check_first_runtime_path(run, 12, &runtime, &binding).is_err());
    let mut displaced = binding;
    displaced["path"] = json!(
        run.join("lifecycle-0012")
            .join("redis")
            .join("runtime.json")
    );
    assert!(check_first_runtime_path(run, 8, &runtime, &displaced).is_err());
    displaced["path"] = Value::Null;
    assert!(check_first_runtime_path(run, 8, &runtime, &displaced).is_err());
}

#[test]
#[cfg(windows)]
fn windows_request_binding_accepts_mixed_separators_without_accepting_other_files() {
    let request = json!({"path":r"D:\fixture\lifecycle-0008\redis/request.json",
        "bytes":123,"sha256":"a".repeat(64)});
    let mut same = request.clone();
    same["path"] = json!(r"D:\fixture\lifecycle-0008\redis\request.json");
    assert!(successor::same_request(&request, &same).unwrap());
    for (field, value) in [
        (
            "path",
            json!(r"D:\fixture\lifecycle-0012\redis\request.json"),
        ),
        ("bytes", json!(124)),
        ("sha256", json!("b".repeat(64))),
    ] {
        let mut changed = same.clone();
        changed[field] = value;
        assert!(
            !successor::same_request(&request, &changed).unwrap(),
            "{field}"
        );
    }
}

#[test]
fn second_restart_requires_a_unique_passed_reconcile_row() {
    let result = json!({"path":"close-reconcile"});
    let row =
        json!({"stage":"fixture-services","mode":"reconcile","status":"passed","result":result});
    assert_eq!(
        close_row(std::slice::from_ref(&row), &result).unwrap(),
        &row
    );
    assert!(close_row(&[row.clone(), row.clone()], &result).is_err());
    let mut failed = row;
    failed["status"] = json!("failed");
    assert!(close_row(&[failed], &result).is_err());
}

#[test]
fn copy_stage_allows_the_interrupted_prefix_and_its_successful_read_only_reconciliation() {
    let verification = json!({"number":1,"stage":"target-verify","mode":"run","status":"passed"});
    let interrupted = json!({"number":2,"stage":"copy","mode":"run","status":"failed","error_type":"ControllerInterrupted"});
    let reconcile = json!({"number":3,"stage":"copy","mode":"reconcile","status":"passed"});
    assert!(eligible_attempts(&[verification.clone(), interrupted.clone()]).is_ok());
    assert!(
        eligible_attempts(&[verification.clone(), interrupted.clone(), reconcile.clone()]).is_ok()
    );
    let mut failed = reconcile.clone();
    failed["status"] = json!("failed");
    assert!(eligible_attempts(&[verification.clone(), interrupted.clone(), failed]).is_err());
    let mut completed = reconcile;
    completed["mode"] = json!("resume");
    assert!(eligible_attempts(&[verification, interrupted, completed]).is_err());
}
