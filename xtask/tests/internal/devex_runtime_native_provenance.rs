use super::*;

fn sample(pid: u32) -> Value {
    json!({"ok":true,"receipt":{"format_version":3,"kind":"devex-runtime-provenance",
        "runtime":{"generation":3},"processes":{"api":{"identity":{"pid":pid}}}}})
}

fn read(directory: &Path, phase: &str) -> Value {
    serde_json::from_slice(&fs::read(directory.join(format!("provenance-{phase}.json"))).unwrap())
        .unwrap()
}

#[test]
fn phase_receipts_preserve_exact_process_identity_after_measurement() {
    for changed in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let before = provenance::phase_receipt(
            directory.path(),
            "before",
            Duration::ZERO,
            &Value::Null,
            Ok(sample(101)),
        )
        .unwrap();
        let pid = if changed { 102 } else { 101 };
        let after = provenance::phase_receipt(
            directory.path(),
            "after",
            Duration::from_millis(1),
            &before,
            Ok(sample(pid)),
        );
        if changed {
            assert_eq!(after.unwrap_err().category, "provenance_sample_changed");
        } else {
            assert_eq!(after.unwrap(), before);
        }
        assert_eq!(read(directory.path(), "before")["success"], true);
        let after = read(directory.path(), "after");
        assert_eq!(after["success"], !changed);
        assert_eq!(after["duration_ms"], 1.0);
        assert_eq!(after["receipt"], sample(pid)["receipt"]);
    }
}

#[test]
fn invalid_verifier_response_keeps_only_the_fixed_failure_category() {
    for response in [
        Ok(json!({"ok":false,"stage":"private-error-body"})),
        Ok(json!({"ok":true,"receipt":{"format_version":2,"kind":"old"}})),
        Err(failure::Failure::new("process_timeout")),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let category = if response.is_ok() {
            "provenance_verifier_result"
        } else {
            "process_timeout"
        };
        let failure = provenance::phase_receipt(
            directory.path(),
            "before",
            Duration::ZERO,
            &Value::Null,
            response,
        )
        .unwrap_err();
        assert_eq!(failure.category, category);
        let receipt = read(directory.path(), "before");
        assert_eq!(receipt["success"], false);
        assert_eq!(receipt["failure"], category);
        assert!(receipt["receipt"].is_null());
        assert!(!receipt.to_string().contains("private-error-body"));
        assert!(!directory.path().join("provenance-after.json").exists());
    }
}

#[test]
fn source_request_is_frozen_and_excludes_the_retired_interpreter() {
    let directory = tempfile::tempdir().unwrap();
    let input = input(directory.path());
    let descriptor = json!({"path":directory.path().join("receipt.json"),
        "bytes":1,"sha256":"a".repeat(64)});
    let mut config = json!({"contract":{"environment_sha256":"b".repeat(64)},"bindings":{
        "scope_id":"provenance-fixture","source_fingerprints":input.source_fingerprints,
        "api_url":"http://127.0.0.1:18080","frontend_url":"http://127.0.0.1:4174",
        "metrics_urls":{"api":"http://127.0.0.1:18080/api/v1/monitor/metrics","worker":"http://127.0.0.1:18081/metrics"},
        "provenance":{"runtime_receipt":descriptor,"target_plan":descriptor,
            "runtime_registration":descriptor,"runtime_launch":descriptor,
            "environment_document":directory.path().join("environment.json")}}});
    let original = provenance::specification(&config, &input).unwrap();
    config["bindings"]["scope_id"] = json!("changed-after-start");
    config["bindings"]["provenance"]["python"] = json!(directory.path().join("python.exe"));
    assert!(provenance::specification(&config, &input).is_err());
    assert_eq!(original["scope_id"], "provenance-fixture");
    assert_eq!(original["driver"], json!(input.driver));
    assert_eq!(
        original["runner_frontend_fingerprint"],
        input.runner_frontend_fingerprint
    );
    assert!(original["provenance"].get("python").is_none());
    assert!(original.get("native_tool").is_none());
}
