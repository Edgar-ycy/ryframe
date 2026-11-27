use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use tempfile::TempDir;

use super::{collect_bound, contract};
use crate::recovery::{
    clone::native::{
        cache::handoff::history::{Producer, active},
        digest,
        resources::common,
        target_verify::input::request_configuration::python_configuration_hash,
    },
    full_stack::worker::role::Role,
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
        let binding = common::binding(&self.runtime.join("runtime.json"))?;
        collect_bound(&self.backend, &self.backend, &self.runtime, &binding)
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
        (
            "api_config_dir",
            json!(fixture.backend.join("unbound/config")),
        ),
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

fn publish_runtime_binaries(fixture: &Fixture) {
    let api = &fixture.receipt["artifacts"]["api"]["path"];
    let worker = &fixture.receipt["artifacts"]["worker"]["path"];
    fs::write(
        fixture.runtime.join("binaries.json"),
        serde_json::to_vec(&json!({
            "ryframe":api,"ryframe-worker":worker,"ryframe-reset":api,"ryframe-migrate":api
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn native_producer_projects_both_roles_and_keeps_exact_repeated_events() {
    use crate::recovery::clone::native::seed::runtime_pair::StopKind;
    let fixture = Fixture::new();
    publish_runtime_binaries(&fixture);
    let history = fixture.runtime.join("producer-history.json");
    let api_url = "http://127.0.0.1:18210/readyz";
    let worker_url = fixture.receipt["worker_ready_url"].as_str().unwrap();
    let api_token = "b".repeat(32);
    let worker_token = "c".repeat(32);
    let api = Producer::new(
        &fixture.backend,
        &fixture.runtime,
        &fixture.receipt,
        Role::Api,
        &api_token,
        api_url,
    )
    .unwrap();
    let worker = Producer::new(
        &fixture.backend,
        &fixture.runtime,
        &fixture.receipt,
        Role::Worker,
        &worker_token,
        worker_url,
    )
    .unwrap();

    assert_pending_api_intent(&fixture, &history, &api, &worker);

    let api_identity = json!({"pid":424243,"started":"12345678901234567891",
        "executable":fixture.receipt["artifacts"]["api"]["path"]});
    let worker_identity = json!({"pid":424244,"started":"12345678901234567892",
        "executable":fixture.receipt["artifacts"]["worker"]["path"]});
    for (role, token, identity, producer) in [
        ("api", &api_token, &api_identity, &api),
        ("worker", &worker_token, &worker_identity, &worker),
    ] {
        producer.intent().unwrap();
        fs::write(fixture.runtime.join(format!("{role}-{token}.log")), "").unwrap();
        fs::write(fixture.runtime.join(format!("{role}.json")), serde_json::to_vec(&json!({
            "format_version":1,"role":role,"scope_id":fixture.receipt["scope_id"],"identity":identity
        })).unwrap()).unwrap();
        producer.started(identity).unwrap();
        let started = fs::read(&history).unwrap();
        producer.started(identity).unwrap();
        assert_eq!(fs::read(&history).unwrap(), started);
    }

    assert!(worker.started(&api_identity).is_err());
    for (role, token, identity, url) in [
        (Role::Api, &api_token, &api_identity, api_url),
        (Role::Worker, &worker_token, &worker_identity, worker_url),
    ] {
        let selected = active(&fixture.backend, &fixture.backend, &fixture.runtime, role)
            .unwrap()
            .unwrap();
        assert_eq!(selected.token, *token);
        assert_eq!(selected.identity, *identity);
        assert_eq!(selected.ready_url, url);
    }
    assert!(worker.stopped(&api_identity, || Ok(())).is_err());
    worker.stopped(&worker_identity, || Ok(())).unwrap();
    StopKind::StartupRollback
        .publish(&api, &api_identity, || Ok(()))
        .unwrap();
    let closed = fs::read(&history).unwrap();
    worker.stopped(&worker_identity, || Ok(())).unwrap();
    StopKind::StartupRollback
        .publish(&api, &api_identity, || Ok(()))
        .unwrap();
    assert!(api.stopped(&api_identity, || Ok(())).is_err());
    assert_eq!(fs::read(&history).unwrap(), closed);
    for role in [Role::Api, Role::Worker] {
        assert!(
            active(&fixture.backend, &fixture.backend, &fixture.runtime, role)
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(
        fixture.read().unwrap()["generations"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

fn assert_pending_api_intent(
    fixture: &Fixture,
    history: &Path,
    api: &Producer<'_>,
    worker: &Producer<'_>,
) {
    api.intent().unwrap();
    let pending = fs::read(history).unwrap();
    api.intent().unwrap();
    assert!(worker.intent().is_err());
    assert!(fixture.read().is_err());
    assert!(
        active(
            &fixture.backend,
            &fixture.backend,
            &fixture.runtime,
            Role::Api
        )
        .is_err()
    );
    assert_eq!(fs::read(history).unwrap(), pending);
}

#[test]
fn verified_external_product_is_separate_from_the_history_directory() {
    let mut fixture = Fixture::new();
    let product = fixture.backend.join(".local-tests/product");
    fs::create_dir(&product).unwrap();
    let product = super::canonical(&product).unwrap();
    fixture.receipt["backend_root"] = json!(product);
    fixture.history["events"]
        .as_array_mut()
        .unwrap()
        .pop()
        .unwrap();
    fixture.publish();
    assert!(fixture.read().is_err());
    let binding = common::binding(&fixture.runtime.join("runtime.json")).unwrap();
    let lineage = collect_bound(&fixture.backend, &product, &fixture.runtime, &binding).unwrap();
    assert_eq!(lineage["runtime"], binding);
    use crate::recovery::{
        clone::native::cache::handoff::history::active, full_stack::worker::role::Role,
    };
    let selected = active(&fixture.backend, &product, &fixture.runtime, Role::Api)
        .unwrap()
        .unwrap();
    assert_eq!(selected.identity, fixture.role["identity"]);
    assert!(
        active(
            &fixture.backend,
            &fixture.backend,
            &fixture.runtime,
            Role::Api
        )
        .is_err()
    );
    let other = fixture.backend.join(".local-tests/other-product");
    fs::create_dir(&other).unwrap();
    assert!(collect_bound(&fixture.backend, &other, &fixture.runtime, &binding).is_err());
    let copied = fixture.runtime.join("runtime-copy.json");
    fs::copy(fixture.runtime.join("runtime.json"), &copied).unwrap();
    let misplaced = common::binding(&copied).unwrap();
    assert!(collect_bound(&fixture.backend, &product, &fixture.runtime, &misplaced).is_err());
    fs::write(
        fixture.runtime.join("runtime.json"),
        serde_json::to_vec_pretty(&fixture.receipt).unwrap(),
    )
    .unwrap();
    assert!(collect_bound(&fixture.backend, &product, &fixture.runtime, &binding).is_err());
}

#[test]
fn source_lineage_collects_published_post_and_seed_contracts_for_the_external_product() {
    use crate::recovery::clone::native::{json as document, lifecycle::state, seed};

    let mut fixture = Fixture::new();
    let product = fixture.backend.join(".local-tests/product");
    fs::create_dir(&product).unwrap();
    let product = super::canonical(&product).unwrap();
    fixture.receipt["backend_root"] = json!(product);
    fixture.publish();
    publish_runtime_binaries(&fixture);
    let run = fixture.runtime.parent().unwrap().parent().unwrap();
    let write = |path: &Path, value: &Value| {
        document::write_new(path, value).unwrap();
        common::binding(path).unwrap()
    };
    let build = write(&run.join("product.json"), &json!({"fixture":true}));
    write(&run.join("manifest.json"), &json!({"fixture":true}));
    state::initialize(run).unwrap();
    let stage = |name, mode, result: &Value| {
        let (number, _) = state::begin(run, name, mode, &json!({})).unwrap();
        state::finish(run, number, Some(result), None).unwrap();
    };
    let post = json!({"backend_build":build});
    let registration = write(&run.join("post-copy.json"), &post);
    stage(
        "post-copy",
        "register",
        &json!({"status":"post_copy_registered","registration":registration}),
    );
    let runtime = common::binding(&fixture.runtime.join("runtime.json")).unwrap();
    let binaries = common::binding(&fixture.runtime.join("binaries.json")).unwrap();
    let handoff = write(
        &fixture.runtime.join("handoff.json"),
        &json!({
            "registration":registration,"runtime":runtime,"binaries":binaries,"worker_started":false
        }),
    );
    stage(
        "post-copy",
        "prepare",
        &json!({"status":"post_copy_api_prepared","handoff":handoff,"worker_started":false}),
    );
    let seed_runtime = run.join("seed-runtime/runtime");
    fs::create_dir_all(&seed_runtime).unwrap();
    let seed_contract = write(&seed_runtime.join("runtime.json"), &fixture.receipt);
    let intent = &fixture.history["events"][0];
    fs::write(seed_runtime.join(intent["log"].as_str().unwrap()), "").unwrap();
    write(
        &seed_runtime.join("producer-history.json"),
        &json!({
            "format_version":1,"kind":"devex-clone-producer-history","scope_id":"fixture-scope",
            "runtime_directory":seed_runtime,"events":[intent,
                {"sequence":1,"time_ns":101,"event":"start-failed","role":"api","token":intent["token"],
                "error_type":"RuntimeError","log":intent["log"]}]
        }),
    );
    stage("seed-runtime", "prepare", &json!({"fixture":true}));
    let saved = state::load(run, true).unwrap();
    let post = seed::history::post(&fixture.backend, run, None, None, true).unwrap();
    let inputs = seed::runtime_model::Inputs {
        runtime: seed_runtime,
        post_copy_runtime: post.runtime.clone(),
        handoff: json!({"backend_build":build,"runtime":seed_contract}),
        environment: json!({"APP_ENV":"test"}),
    };
    let collect = |product| {
        super::super::collect(
            &fixture.backend,
            run,
            &Value::Null,
            product,
            &saved,
            &post,
            &inputs,
        )
    };
    let records = collect(&product).unwrap();
    assert_eq!(records["runtimes"][0]["runtime"], runtime);
    assert_eq!(records["runtimes"][1]["runtime"], seed_contract);
    assert!(collect(&fixture.backend).is_err());
    fs::write(inputs.runtime.join("runtime.json"), "{}").unwrap();
    assert!(collect(&product).is_err());
}

#[test]
fn native_producer_keeps_unknown_start_even_when_a_log_exists() {
    use crate::recovery::{
        clone::native::{
            cache::handoff::history::{Producer, pending_start},
            resources::common,
        },
        full_stack::worker::role::Role,
    };
    let fixture = Fixture::new();
    publish_runtime_binaries(&fixture);
    let token = "d".repeat(32);
    let producer = Producer::new(
        &fixture.backend,
        &fixture.runtime,
        &fixture.receipt,
        Role::Worker,
        &token,
        fixture.receipt["worker_ready_url"].as_str().unwrap(),
    )
    .unwrap();
    let history = fixture.runtime.join("producer-history.json");
    producer.intent().unwrap();
    let pending = fs::read(&history).unwrap();
    assert!(fixture.read().is_err());

    fs::write(fixture.runtime.join(format!("worker-{token}.log")), "").unwrap();
    let intent = pending_start(&fixture.backend, &fixture.runtime, &fixture.receipt)
        .unwrap()
        .unwrap();
    assert_eq!(intent.role, Role::Worker);
    assert_eq!(intent.token, token);
    assert_eq!(
        intent.ready_url,
        fixture.receipt["worker_ready_url"].as_str().unwrap()
    );
    assert_eq!(intent.binding, common::binding(&history).unwrap());
    assert!(
        producer
            .stopped(&fixture.role["identity"], || Ok(()))
            .is_err()
    );
    assert_eq!(fs::read(&history).unwrap(), pending);
    assert!(fixture.read().is_err());
}

#[test]
fn missing_history_does_not_hide_orphaned_role_evidence() {
    use crate::recovery::{
        clone::native::cache::handoff::history::active, full_stack::worker::role::Role,
    };
    let fixture = Fixture::new();
    publish_runtime_binaries(&fixture);
    let observe = || {
        active(
            &fixture.backend,
            &fixture.backend,
            &fixture.runtime,
            Role::Worker,
        )
    };
    fs::remove_file(fixture.runtime.join("producer-history.json")).unwrap();
    assert!(observe().is_err());
    fs::remove_file(fixture.runtime.join("api.json")).unwrap();
    fs::remove_file(fixture.runtime.join(format!("api-{}.log", "a".repeat(32)))).unwrap();
    assert!(observe().unwrap().is_none());
    let lock = fixture.runtime.join("worker-control.lock");
    fs::create_dir(&lock).unwrap();
    assert!(observe().is_err());
    assert!(lock.is_dir());
}
