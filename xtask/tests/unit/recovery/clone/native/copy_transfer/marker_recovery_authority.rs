use super::*;

#[test]
fn finished_external_restart_must_be_the_only_active_fixture_successor() {
    let first = json!({"stage":"fixture-services","mode":"restart","status":"passed",
        "result":{"path":"original-restart"}});
    let failed = json!({"stage":"fixture-services","mode":"restart","status":"failed"});
    let mut history = json!({"active_generation":first["result"],"closed":false,
        "external_recovery":null,"rustfs_handoff":null});
    assert_eq!(
        original_restart(&history, &[failed, first.clone()]).unwrap(),
        &first
    );
    assert!(original_restart(&history, &[first.clone(), first.clone()]).is_err());
    history["active_generation"] = json!({"path":"different"});
    assert!(original_restart(&history, &[first.clone()]).is_err());
    history["active_generation"] = first["result"].clone();
    history["external_recovery"] = json!({"path":"newer-external-termination"});
    assert!(original_restart(&history, &[first]).is_err());
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
    check_redis_successor(
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
            check_redis_successor(
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
        check_redis_successor(
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
