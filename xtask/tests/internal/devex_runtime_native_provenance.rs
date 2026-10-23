use super::*;

struct Verifier {
    root: tempfile::TempDir,
    config: Value,
    input: Input,
}

impl Verifier {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut input = input(root.path());
        input.driver = root.path().into();
        let python =
            crate::check::consumer_context::python::selected_program(&crate::workspace::root_dir())
                .unwrap();
        let scripts = root.path().join("tools/python");
        fs::create_dir_all(&scripts).unwrap();
        fs::write(
            scripts.join("devex_provenance.py"),
            r#"import json, pathlib, sys
request = json.load(sys.stdin)
with pathlib.Path('requests.jsonl').open('a', encoding='utf-8') as output:
    output.write(json.dumps(request, sort_keys=True) + '\n')
print(pathlib.Path('response.json').read_text(encoding='utf-8'))
"#,
        )
        .unwrap();
        let descriptor =
            json!({"path":root.path().join("receipt.json"),"bytes":1,"sha256":"a".repeat(64)});
        let config = json!({"contract":{"environment_sha256":"b".repeat(64)},"bindings":{
            "scope_id":"provenance-fixture","source_fingerprints":input.source_fingerprints,
            "api_url":"http://127.0.0.1:18080","frontend_url":"http://127.0.0.1:4174",
            "metrics_urls":["http://127.0.0.1:18081/metrics"],
            "provenance":{"python":python,"runtime_receipt":descriptor,"target_plan":descriptor,
                "runtime_registration":descriptor,"runtime_launch":descriptor,
                "environment_document":root.path().join("environment.json")}}});
        let fixture = Self {
            root,
            config,
            input,
        };
        fixture.response(json!({"ok":true,"receipt":{"format_version":3,
            "kind":"devex-runtime-provenance","api_identity":{"pid":101}}}));
        fixture
    }

    fn response(&self, response: Value) {
        fs::write(
            self.root.path().join("response.json"),
            serde_json::to_vec(&response).unwrap(),
        )
        .unwrap();
    }

    fn receipt(&self, phase: &str) -> Value {
        serde_json::from_slice(
            &fs::read(self.root.path().join(format!("provenance-{phase}.json"))).unwrap(),
        )
        .unwrap()
    }
}

#[test]
fn verifier_freezes_request_and_rechecks_process_identity_after_measurement() {
    asynchronous(async {
        for changed in [false, true] {
            let mut fixture = Verifier::new();
            let cancel = process::Cancellation::default();
            let mut guard = provenance::Guard::before(
                &fixture.config,
                &fixture.input,
                fixture.root.path(),
                &cancel,
            )
            .await
            .unwrap();
            fixture.config["bindings"]["scope_id"] = json!("changed-after-start");
            if changed {
                fixture.response(json!({"ok":true,"receipt":{"format_version":3,
                    "kind":"devex-runtime-provenance","api_identity":{"pid":102}}}));
            }
            let result = guard.after(&cancel).await;
            if changed {
                assert_eq!(result.unwrap_err().category, "provenance_sample_changed");
            } else {
                result.unwrap();
            }
            let before = fixture.receipt("before");
            let after = fixture.receipt("after");
            assert_eq!(before["success"], true);
            assert_eq!(after["success"], !changed);
            assert!(after["duration_ms"].as_f64().unwrap() >= 0.0);
            let raw = fs::read_to_string(fixture.root.path().join("requests.jsonl")).unwrap();
            let requests = raw
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0], requests[1]);
            assert_eq!(requests[0]["scope_id"], "provenance-fixture");
            assert_eq!(requests[0]["driver"], json!(fixture.input.driver));
            assert_eq!(
                requests[0]["runner_frontend_fingerprint"],
                fixture.input.runner_frontend_fingerprint
            );
        }
    });
}

#[test]
fn verifier_invalid_response_preserves_failure_receipt_without_private_error_body() {
    asynchronous(async {
        for response in [
            json!({"ok":false,"stage":"private-error-body"}),
            json!({"ok":true,"receipt":{"format_version":2,"kind":"old"}}),
        ] {
            let fixture = Verifier::new();
            let cancel = process::Cancellation::default();
            fixture.response(response.clone());
            let failure = provenance::Guard::before(
                &fixture.config,
                &fixture.input,
                fixture.root.path(),
                &cancel,
            )
            .await
            .err()
            .unwrap();
            assert_eq!(failure.category, "provenance_verifier_result");
            let receipt = fixture.receipt("before");
            assert_eq!(receipt["success"], false);
            assert_eq!(receipt["failure"], "provenance_verifier_result");
            assert!(!receipt.to_string().contains("private-error-body"));
            assert!(!fixture.root.path().join("provenance-after.json").exists());
        }
    });
}
