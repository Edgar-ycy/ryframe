use super::*;

#[test]
fn missing_job_proof_excludes_success_but_preserves_duration_and_redacted_evidence() {
    asynchronous(async {
        let temp = tempfile::tempdir().unwrap();
        let cycle = |elapsed, proof| load::Cycle {
            variables: json!({"worker":0,"cycle":0,"tenant_id":"private-tenant",
                "username":"private-user","token":"private-token"}),
            succeeded: true,
            request_elapsed: 7.0,
            elapsed,
            completion: None,
            proof,
            failure: None,
            queue: None,
            execution: None,
        };
        let mut measurement = load::Measurement {
            scenarios: BTreeMap::new(),
            network_requests: 2,
            completed_cycles: 0,
            failed_cycles: 0,
            session_failures: 0,
            pending: vec![
                load::Pending {
                    workflow: json!({"name":"list"}),
                    records: vec![cycle(10.0, true)],
                    failed: 0,
                    elapsed: 20.0,
                    events: vec![],
                },
                load::Pending {
                    // 缺少业务 ID，不能查询数据库或凭已完成请求认定作业通过。
                    workflow: json!({"name":"export","job":{"kind":"export",
                        "id":"${export_id}","job_type":"system.export.execute"}}),
                    records: vec![cycle(99.0, false)],
                    failed: 0,
                    elapsed: 120.0,
                    events: vec![],
                },
            ],
        };
        measurement
            .finalize(&json!({}), temp.path(), &process::Cancellation::default())
            .await
            .unwrap();
        assert_eq!(measurement.completed_cycles, 1);
        assert_eq!(measurement.failed_cycles, 1);
        assert_eq!(measurement.scenarios["list"]["p95_ms"], 10.0);
        assert_eq!(measurement.scenarios["export"]["completed"], 0);
        assert_eq!(measurement.scenarios["export"]["failed"], 1);
        assert_eq!(measurement.scenarios["export"]["p95_ms"], 0.0);
        assert_eq!(measurement.scenarios["export"]["elapsed_ms"], 120.0);
        let cycles = fs::read_to_string(temp.path().join("cycles.jsonl")).unwrap();
        let rows = cycles
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1]["business_duration_ms"], 99.0);
        assert_eq!(rows[1]["request_duration_ms"], 7.0);
        assert_eq!(rows[1]["succeeded"], true);
        assert_eq!(rows[1]["proof"], false);
        assert_eq!(
            rows[1]["proof_failure"]["category"],
            "job_timing_evidence_invalid"
        );
        let attempts = fs::read_to_string(temp.path().join("job-attempts.jsonl")).unwrap();
        let attempt: Value = serde_json::from_str(attempts.trim()).unwrap();
        assert_eq!(attempt["valid"], false);
        assert_eq!(attempt["business_duration_ms"], 99.0);
        for secret in ["private-tenant", "private-user", "private-token"] {
            assert!(!cycles.contains(secret));
            assert!(!attempts.contains(secret));
        }
    });
}

#[test]
fn import_preparation_failure_records_receipt_and_cancellation_creates_no_samples() {
    let config = |directory: &Path| {
        json!({"contract":{"cycles":2,"workloads":{"jobs":[{"job":{"kind":"import"}}]},
            "import_samples":{"template_sha256":"a".repeat(64),
                "model_sha256":crate::devex::import_samples::model_sha256().unwrap()}},
            "bindings":{"import_samples":{"template":directory.join("missing.xlsx")}}})
    };
    let failed = tempfile::tempdir().unwrap();
    let error = imports::prepare(
        &config(failed.path()),
        "jobs",
        2,
        failed.path(),
        &process::Cancellation::default(),
    )
    .err()
    .unwrap();
    assert_eq!(error.category, "import_sample_preparation_failed");
    let receipt: Value =
        serde_json::from_slice(&fs::read(failed.path().join("import-preparation.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["success"], false);
    assert_eq!(receipt["failure"], "import_sample_preparation_failed");
    assert!(receipt["manifest"].is_null());
    assert!(receipt["duration_ms"].as_f64().unwrap() >= 0.0);
    assert_eq!(fs::read_dir(failed.path()).unwrap().count(), 1);
    let cancelled = tempfile::tempdir().unwrap();
    let cancel = process::Cancellation::default();
    cancel.cancel();
    let error = imports::prepare(
        &config(cancelled.path()),
        "jobs",
        2,
        cancelled.path(),
        &cancel,
    )
    .err()
    .unwrap();
    assert_eq!(error.category, "request_cancelled");
    assert_eq!(fs::read_dir(cancelled.path()).unwrap().count(), 0);
}

#[test]
fn invalid_provenance_records_failure_without_starting_verifier_or_exposing_bindings() {
    asynchronous(async {
        let temp = tempfile::tempdir().unwrap();
        let config = json!({"bindings":{"provenance":{"python":"private-credential"}}});
        let error = provenance::Guard::before(
            &config,
            &input(temp.path()),
            temp.path(),
            &process::Cancellation::default(),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error.category, "provenance_bindings");
        let raw = fs::read_to_string(temp.path().join("provenance-before.json")).unwrap();
        let receipt: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(receipt["phase"], "before");
        assert_eq!(receipt["success"], false);
        assert_eq!(receipt["failure"], "provenance_bindings");
        assert!(!raw.contains("private-credential"));
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    });
}
