use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use tempfile::TempDir;

use super::{collect, contract};
use crate::recovery::clone::native::{
    digest, target_verify::input::request_configuration::python_configuration_hash,
};

struct Fixture {
    _directory: TempDir,
    backend: PathBuf,
    runtime: PathBuf,
    receipt: Value,
    history: Value,
    role: Value,
}

impl Fixture {
    fn new() -> Self {
        let parent = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&parent).unwrap();
        let holder = tempfile::Builder::new()
            .prefix("runtime-history-")
            .tempdir_in(parent)
            .unwrap();
        let backend = super::canonical(holder.path()).unwrap();
        let runtime = backend.join(".local-tests/run/post-copy/runtime");
        fs::create_dir_all(&runtime).unwrap();
        for (name, id) in [("config", 3), ("worker-config", 2)] {
            let path = runtime.join(name);
            fs::create_dir(&path).unwrap();
            fs::write(path.join("app.toml"), "scope_id='fixture-scope'\n[jobs]\nmode='external'\nhealth_host='127.0.0.1'\nhealth_port=19210\n").unwrap();
            fs::write(
                path.join("app.test.toml"),
                format!("snowflake_worker_id={id}\n"),
            )
            .unwrap();
        }
        let api = backend.join("api.exe");
        let worker = backend.join("worker.exe");
        fs::write(&api, "api fixture").unwrap();
        fs::write(&worker, "worker fixture").unwrap();
        let api_sha = contract::configuration_digest(&backend, &runtime.join("config")).unwrap();
        let worker_sha =
            contract::configuration_digest(&backend, &runtime.join("worker-config")).unwrap();
        let combined = python_configuration_hash(&json!([api_sha, worker_sha])).unwrap();
        let artifact =
            |path: &Path| json!({"path": path, "sha256": digest::file(path).unwrap().sha256});
        let receipt = json!({"format_version":1,"backend_root":backend,"scope_id":"fixture-scope",
            "configuration_sha256":combined,"api_config_dir":runtime.join("config"),
            "worker_config_dir":runtime.join("worker-config"),"api_configuration_sha256":api_sha,
            "worker_configuration_sha256":worker_sha,"worker_ready_url":"http://127.0.0.1:19210/readyz",
            "artifacts":{"api":artifact(&api),"worker":artifact(&worker)}});
        let identity =
            json!({"pid":424242,"started":"12345678901234567890123456789012","executable":api});
        let token = "a".repeat(32);
        let log = format!("api-{token}.log");
        fs::write(runtime.join(&log), "").unwrap();
        let history = json!({"format_version":1,"kind":"devex-clone-producer-history","scope_id":"fixture-scope",
            "runtime_directory":runtime,"events":[
                {"sequence":0,"time_ns":100,"event":"start-intent","role":"api","token":token,
                    "artifact":receipt["artifacts"]["api"],"ready_url":"http://127.0.0.1:18210/readyz",
                    "configuration_sha256":combined,"log":log},
                {"sequence":1,"time_ns":101,"event":"started","role":"api","token":token,"identity":identity},
                {"sequence":2,"time_ns":102,"event":"stopped","role":"api","identity":identity}]});
        let role =
            json!({"format_version":1,"role":"api","scope_id":"fixture-scope","identity":identity});
        let fixture = Self {
            _directory: holder,
            backend,
            runtime,
            receipt,
            history,
            role,
        };
        fixture.publish();
        fixture
    }

    fn publish(&self) {
        for (name, value) in [
            ("runtime.json", &self.receipt),
            ("producer-history.json", &self.history),
            ("api.json", &self.role),
        ] {
            fs::write(self.runtime.join(name), serde_json::to_vec(value).unwrap()).unwrap();
        }
    }

    fn read(&self) -> crate::Result<Value> {
        collect(&self.backend, &self.runtime)
    }
}

#[test]
fn complete_single_runtime_keeps_full_creation_time_and_empty_log_without_live_checks() {
    let fixture = Fixture::new();
    let result = fixture.read().unwrap();
    assert_eq!(result.as_object().unwrap().len(), 6);
    assert_eq!(
        result["runtime_directory"],
        json!(super::canonical(&fixture.runtime).unwrap())
    );
    assert_eq!(result["generations"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["generations"][0]["identity"],
        fixture.role["identity"]
    );
    assert_eq!(
        result["role_receipts"][0]["identity"],
        fixture.role["identity"]
    );
    assert_eq!(
        result["runtime"]["sha256"],
        digest::file(&fixture.runtime.join("runtime.json"))
            .unwrap()
            .sha256
    );
    fs::remove_file(fixture.runtime.join("api.json")).unwrap();
    assert!(fixture.read().is_err());
}

#[test]
fn orphan_intent_and_started_after_failure_are_rejected() {
    let mut fixture = Fixture::new();
    let original = fixture.history["events"].as_array().unwrap().clone();
    fixture.history["events"] = json!([original[0]]);
    fixture.publish();
    assert!(fixture.read().is_err());
    let failed = json!({"sequence":1,"time_ns":101,"event":"start-failed","role":"api",
        "token":original[0]["token"],"error_type":"RuntimeError","log":original[0]["log"]});
    let mut late = original[1].clone();
    late["sequence"] = json!(2);
    late["time_ns"] = json!(102);
    fixture.history["events"] = json!([original[0], failed, late]);
    fixture.publish();
    assert!(fixture.read().is_err());
    fixture.history["events"] = json!([original[0], failed]);
    fixture.publish();
    fs::remove_file(fixture.runtime.join("api.json")).unwrap();
    assert!(
        fixture.read().unwrap()["generations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn duplicate_identity_double_close_and_close_after_failure_are_rejected() {
    let mut fixture = Fixture::new();
    let original = fixture.history["events"].as_array().unwrap().clone();
    let mut intent = original[0].clone();
    let token = "b".repeat(32);
    let log = format!("api-{token}.log");
    fs::write(fixture.runtime.join(&log), "").unwrap();
    intent["token"] = json!(token);
    intent["log"] = json!(log);
    intent["sequence"] = json!(3);
    intent["time_ns"] = json!(103);
    let mut started = original[1].clone();
    started["token"] = json!(token);
    started["sequence"] = json!(4);
    started["time_ns"] = json!(104);
    fixture.history["events"] = json!([original[0], original[1], original[2], intent, started]);
    fixture.publish();
    assert!(fixture.read().is_err());
    let mut close = original[2].clone();
    close["sequence"] = json!(3);
    close["time_ns"] = json!(103);
    fixture.history["events"] = json!([original[0], original[1], original[2], close]);
    fixture.publish();
    assert!(fixture.read().is_err());
    let failed = json!({"sequence":2,"time_ns":102,"event":"start-failed","role":"api",
        "token":original[0]["token"],"error_type":"RuntimeError","log":original[0]["log"]});
    fixture.history["events"] = json!([original[0], original[1], failed, close]);
    fixture.publish();
    assert!(fixture.read().is_err());
}

#[test]
fn rollback_and_failed_started_generation_keep_latest_role_ownership() {
    let mut fixture = Fixture::new();
    fixture.history["events"][2]["event"] = json!("startup-rollback");
    fixture.publish();
    assert!(fixture.read().is_ok());
    let old = fixture.history["events"][0].clone();
    fixture.history["events"][2] = json!({"sequence":2,"time_ns":102,"event":"start-failed","role":"api",
        "token":old["token"],"error_type":"StartupError","log":old["log"]});
    fixture.publish();
    assert!(fixture.read().is_ok());
    fixture.role["identity"]["started"] = json!("200");
    fixture.publish();
    assert!(fixture.read().is_err());
}

#[test]
fn runtime_configuration_artifact_and_worker_url_drift_fail_closed() {
    let mut fixture = Fixture::new();
    let original = fixture.receipt.clone();
    for (field, replacement) in [
        ("configuration_sha256", json!("f".repeat(64))),
        ("worker_configuration_sha256", json!("d".repeat(64))),
        ("worker_ready_url", json!("http://127.0.0.1/readyz")),
        (
            "worker_ready_url",
            json!("http://user@127.0.0.1:19210/readyz?token=hidden"),
        ),
    ] {
        fixture.receipt = original.clone();
        fixture.receipt[field] = replacement;
        fixture.publish();
        assert!(fixture.read().is_err(), "{field}");
    }
    fixture.receipt = original.clone();
    fixture.receipt["artifacts"]["api"]["sha256"] = json!("e".repeat(64));
    fixture.publish();
    assert!(fixture.read().is_err());
    fixture.receipt = original;
    fixture.publish();
    fs::write(
        fixture.runtime.join("worker-config/app.test.toml"),
        "snowflake_worker_id=9\n",
    )
    .unwrap();
    assert!(fixture.read().is_err());
}

#[test]
fn sequence_time_unknown_fields_and_role_receipt_misalignment_are_rejected() {
    let mut fixture = Fixture::new();
    let original = fixture.history.clone();
    for (field, value) in [
        ("sequence", json!(9)),
        ("time_ns", json!(0)),
        ("time_ns", json!(99)),
        ("role", json!("frontend")),
        ("unknown", json!(true)),
    ] {
        fixture.history = original.clone();
        fixture.history["events"][1][field] = value;
        fixture.publish();
        assert!(fixture.read().is_err(), "{field}");
    }
    fixture.history = original;
    fixture.publish();
    fs::write(
        fixture.runtime.join("worker.json"),
        serde_json::to_vec(&fixture.role).unwrap(),
    )
    .unwrap();
    assert!(fixture.read().is_err());
}

#[test]
fn newest_generation_receipt_is_required_but_local_history_does_not_prove_global_stop() {
    let mut fixture = Fixture::new();
    let mut intent = fixture.history["events"][0].clone();
    let mut started = fixture.history["events"][1].clone();
    let token = "b".repeat(32);
    let log = format!("api-{token}.log");
    fs::write(fixture.runtime.join(&log), "").unwrap();
    intent["token"] = json!(token);
    intent["log"] = json!(log);
    intent["sequence"] = json!(3);
    intent["time_ns"] = json!(103);
    started["token"] = json!(token);
    started["sequence"] = json!(4);
    started["time_ns"] = json!(104);
    started["identity"]["pid"] = json!(424243);
    started["identity"]["started"] = json!("200");
    fixture.history["events"]
        .as_array_mut()
        .unwrap()
        .extend([intent, started.clone()]);
    fixture.publish();
    assert!(fixture.read().is_err());
    fixture.role["identity"] = started["identity"].clone();
    fixture.publish();
    let result = fixture.read().unwrap();
    assert_eq!(result["generations"].as_array().unwrap().len(), 2);
    assert_eq!(result["role_receipts"][0]["identity"], started["identity"]);
}

#[test]
fn large_artifact_is_streamed_while_empty_documents_and_oversized_logs_are_rejected() {
    let mut fixture = Fixture::new();
    let artifact = fixture.backend.join("api.exe");
    fs::OpenOptions::new()
        .write(true)
        .open(&artifact)
        .unwrap()
        .set_len(17 * 1024 * 1024)
        .unwrap();
    let sha = digest::file(&artifact).unwrap().sha256;
    fixture.receipt["artifacts"]["api"]["sha256"] = json!(sha);
    fixture.history["events"][0]["artifact"]["sha256"] = json!(sha);
    fixture.publish();
    assert!(fixture.read().is_ok());
    let log = fixture
        .runtime
        .join(fixture.history["events"][0]["log"].as_str().unwrap());
    fs::OpenOptions::new()
        .write(true)
        .open(&log)
        .unwrap()
        .set_len(16 * 1024 * 1024 + 1)
        .unwrap();
    assert!(fixture.read().is_err());
    fs::write(log, "").unwrap();
    fs::write(fixture.runtime.join("producer-history.json"), "").unwrap();
    assert!(fixture.read().is_err());
}

#[test]
fn configuration_digest_uses_resolved_directory_in_the_python_payload() {
    let fixture = Fixture::new();
    // 保留fixture构造的混合分隔符，以覆盖Windows实际失败路径。
    let selected = fixture.runtime.join("config");
    let resolved = super::canonical(&selected).unwrap();
    let expected = python_configuration_hash(&json!({
        "environment": {"APP_ENV": "test"},
        "config_directory": resolved,
        "files": {
            "app.toml": digest::file(&resolved.join("app.toml")).unwrap().sha256,
            "app.test.toml": digest::file(&resolved.join("app.test.toml")).unwrap().sha256,
        },
    }))
    .unwrap();
    assert_eq!(
        contract::configuration_digest(&fixture.backend, &selected).unwrap(),
        expected
    );
    assert_eq!(
        contract::configuration_digest(&fixture.backend, &resolved).unwrap(),
        expected
    );
    assert_eq!(fixture.receipt["api_configuration_sha256"], expected);
}
