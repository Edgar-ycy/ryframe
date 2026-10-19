use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{
    TargetContext, request, request_configuration, request_evidence, request_physical,
    validate_inputs,
};
use crate::{
    Result,
    recovery::clone::native::{
        digest, input as native_input,
        lifecycle::{manifest, stage::StageRequest},
    },
};

use super::configuration_fixture;

const UUID: &str = "11111111-1111-1111-1111-111111111111";

struct Fixture {
    backend: PathBuf,
    local: PathBuf,
    request: Value,
    review: Value,
    environment: BTreeMap<String, String>,
}

impl Fixture {
    fn new() -> Self {
        let backend = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests")
            .join(format!("target-request-{}", uuid::Uuid::new_v4().simple()));
        let local = backend.join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        fs::create_dir(backend.join("config")).unwrap();
        for tool in ["mysql", "aws"] {
            fs::write(local.join(tool), tool).unwrap();
        }
        let defaults = local.join("mysql.cnf");
        fs::write(&defaults, "[client]\nhost=127.0.0.1\nport=3306\nuser=fixture\npassword=fixture-secret\nssl-mode=DISABLED\n").unwrap();
        let tools = json!({"mysql": tool(&local.join("mysql")), "aws": tool(&local.join("aws"))});
        let review = review(&local, &tools);
        write(&local.join("review.json"), &review);
        write(
            &local.join("build.json"),
            &json!({"kind": "devex-clone-tool-build", "artifacts": {}}),
        );
        let databases = request::KEYS.into_iter().map(|(key, kind, mode)| json!({"key": key, "kind": kind, "mode": mode, "database": database_name("seed", key), "server_uuid": UUID, "defaults_file": defaults, "defaults_sha256": digest::file(&defaults).unwrap().sha256})).collect::<Vec<_>>();
        let mut request = json!({"format_version": 1, "kind": "devex-clone-fresh-target", "id": "fresh-request", "review": null, "side": "seed", "target": {"scope_id": "seed-scope", "s3": {"endpoint": "http://127.0.0.1:29000", "region": "test-region", "access_key_env": "APP_OBJECT_STORAGE_ACCESS_KEY", "secret_key_env": "APP_OBJECT_STORAGE_SECRET_KEY"}, "databases": databases}, "maintenance_build": bound(&local.join("build.json")), "configuration_sha256": "a".repeat(64), "tools": tools, "storage": {"rustfs": {}, "redis": {"port": 26379}}, "reset": {"legacy_ownership": {"mysql_exclusive": true, "redis_exclusive": true, "object_storage_exclusive": true}, "credential_version": "fixture-v1", "sentinel_key": "ryframe:devex-fresh:seed-scope:sentinel", "sentinel_value": "devex-fresh:fresh-request"}});
        request["review"] = review_binding(&local.join("review.json"), &review);
        let mut fixture = Self {
            backend,
            local,
            request,
            review,
            environment: environment(),
        };
        fixture.write_configuration();
        fixture.seal_configuration();
        fixture
    }

    fn write_configuration(&self) {
        let targets = request::KEYS.into_iter().filter(|(key, _, _)| *key != "shared-control").map(|(key, _, mode)| json!({"key": key, "kind": "mysql", "mode": mode, "host": "127.0.0.1", "port": 3306, "database": database_name("seed", key), "username": "fixture", "password_env": "APP_TENANT_PASSWORD", "tls_mode": "disabled"})).collect::<Vec<_>>();
        let table = json!({"app": {"host": "127.0.0.1", "port": 28080}, "database": {"primary": {"host": "127.0.0.1", "port": 3306, "database": database_name("seed", "shared-control"), "username": "fixture", "password": "fixture-secret", "tls_mode": "disabled"}}, "tenant_data": {"targets": targets}, "object_storage": {"backend": "rustfs", "endpoint": "http://127.0.0.1:29000", "region": "test-region", "access_key": "fixture-access", "secret_key": "fixture-secret", "use_ssl": false}, "reset": {"legacy_mysql_exclusive": true, "legacy_redis_exclusive": true, "legacy_object_storage_exclusive": true, "credential_version": "fixture-v1", "redis_outside_sentinel_key": "ryframe:devex-fresh:seed-scope:sentinel"}});
        fs::write(
            self.backend.join("config/app.toml"),
            configuration_fixture::toml_text(&table),
        )
        .unwrap();
    }

    fn seal_review(&mut self) {
        write(&self.local.join("review.json"), &self.review);
        self.request["review"] = review_binding(&self.local.join("review.json"), &self.review);
    }

    fn seal_configuration(&mut self) {
        let (directory, files) =
            request_configuration::configuration_files(&self.backend, &self.environment).unwrap();
        let env = self
            .environment
            .iter()
            .filter(|(key, _)| key.starts_with("APP_"))
            .collect::<BTreeMap<_, _>>();
        let hashes = files
            .iter()
            .map(|(key, value)| (key, &value["sha256"]))
            .collect::<BTreeMap<_, _>>();
        self.request["configuration_sha256"] = json!(request_configuration::python_configuration_hash(&json!({"environment": env, "config_directory": manifest::path_text(&directory, "配置目录").unwrap(), "files": hashes})).unwrap());
    }

    fn validate(&self) -> Result<request::Binding> {
        request::validate(&self.backend, &self.request)
    }

    fn generation(&self) -> Value {
        let binding = self.validate().unwrap();
        let mut result = request_configuration::validate(
            &self.backend,
            &self.request,
            &binding,
            &self.environment,
        )
        .unwrap();
        result["maintenance"] =
            crate::recovery::clone::native::json::read(&self.local.join("build.json")).unwrap();
        result["storage"] = self.request["storage"].clone();
        result["operator_declared_controlled_generation_only"] = json!(true);
        result["external_writers_discovered"] = json!(false);
        result
    }

    fn publish(&self) -> (PathBuf, PathBuf) {
        let target = self.local.join("target");
        let run = self.local.join("run");
        fs::create_dir(&target).unwrap();
        fs::create_dir(&run).unwrap();
        write(&self.local.join("request.json"), &self.request);
        write(&target.join("request.json"), &self.request);
        let generation = self.generation();
        let prepared = prepared(
            &self.request,
            &generation,
            &bound(&self.local.join("request.json")),
        );
        write(&target.join("prepare.json"), &prepared);
        write(
            &target.join("initialize.started.json"),
            &json!({"at": "2026-01-01T00:01:00Z", "generation_sha256": digest::plan_hash(&generation).unwrap()}),
        );
        let initialized = json!({"format_version": 1, "status": "fresh_target_initialized", "id": self.request["id"], "scope_id": self.request["target"]["scope_id"], "completed_at": "2026-01-01T00:02:00Z", "generation": generation, "prepare_sha256": digest::file(&target.join("prepare.json")).unwrap().sha256, "reset": {}, "inventory": {}, "objects": {}, "redis": {}, "history": {}, "controlled_generation_never_started": true, "external_writers_discovered": false, "target_ready": false, "clone_verified": false, "restore_qualified": false});
        write(&target.join("initialized.json"), &initialized);
        write(&target.join("initialized-candidate.json"), &initialized);
        write(
            &self.local.join("target-environment.json"),
            &json!({"environment": self.environment}),
        );
        write(
            &self.local.join("source-environment.json"),
            &json!({"environment": {}}),
        );
        write(
            &self.local.join("source-request.json"),
            &json!({"fixture": true}),
        );
        write(
            &run.join("manifest.json"),
            &json!({"format_version": 1, "kind": "devex-clone-run", "id": "request-input-run", "source_request": bound(&self.local.join("source-request.json")), "source_export": null, "initialized": bound(&target.join("initialized.json")), "source_environment": bound(&self.local.join("source-environment.json")), "target_environment": bound(&self.local.join("target-environment.json")), "copy_directory": self.local.join("copy"), "copy_stage": "source_to_seed", "build_bridges": []}),
        );
        (run, target)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.backend).unwrap();
    }
}

fn environment() -> BTreeMap<String, String> {
    [
        ("APP_ENV", "test"),
        ("APP_SCOPE_ID", "seed-scope"),
        ("APP_JOBS_MODE", "external"),
        ("APP_JOBS_HEALTH_HOST", "127.0.0.1"),
        ("APP_JOBS_HEALTH_PORT", "28081"),
        ("APP_REDIS_HOST", "127.0.0.1"),
        ("APP_REDIS_PORT", "26379"),
        ("APP_REDIS_DATABASE", "0"),
        ("APP_REDIS_TLS", "false"),
        ("APP_TENANT_PASSWORD", "fixture-secret"),
        ("APP_OBJECT_STORAGE_ACCESS_KEY", "fixture-access"),
        ("APP_OBJECT_STORAGE_SECRET_KEY", "fixture-secret"),
        ("RYFRAME_RESET_ADMIN_PASSWORD", "fixture-admin"),
        ("RYFRAME_RESET_USER_PASSWORD", "fixture-user"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect()
}

fn review(local: &Path, tools: &Value) -> Value {
    let mut scopes = serde_json::Map::new();
    for side in ["seed", "base", "candidate"] {
        let scope = format!("{side}-scope");
        let namespace = format!("ryframe:{{{scope}}}:");
        let databases = request::KEYS.into_iter().map(|(key, _, _)| json!({"key": key, "database": database_name(side, key), "expected_server_uuid": UUID, "connection_file": local.join("mysql.cnf"), "host": "127.0.0.1", "port": 3306})).collect::<Vec<_>>();
        scopes.insert(side.into(), json!({"scope_id": scope, "runtime_dir": local.join(format!("{side}-runtime")), "identity_ledger": local.join(format!("{side}-identities")), "api_url": "http://127.0.0.1:28080", "worker_ready_url": "http://127.0.0.1:28081/readyz", "frontend_url": "http://127.0.0.1:28082", "objects": {"endpoint": "http://127.0.0.1:29000", "region": "test-region"}, "redis": {"url": "redis://127.0.0.1:26379/0", "namespace": namespace, "ownership_key": format!("{namespace}.ryframe-owner"), "ownership_value": format!("ryframe-owner:v1:{scope}:redis")}, "databases": databases}));
    }
    let mut tools = tools.clone();
    tools["mysqldump"] = tools["mysql"].clone();
    tools["rustfs"] = tools["aws"].clone();
    tools["redis_server"] = json!({"distribution": "fixture", "resolved_path": "/usr/bin/redis-server", "sha256": "a".repeat(64)});
    json!({"kind": "review-only-perf-resource-plan-with-readonly-preflight", "ready_for_execution": true, "scopes": scopes, "tools": tools, "services": {"rustfs": {"api": "http://127.0.0.1:29000", "console": "http://127.0.0.1:29001", "data_dir": local.join("objects")}, "redis": {"directory": local.join("redis")}}, "reference": {"source": {"databases": [{"server_uuid": UUID, "database": "protected_source"}]}, "protected_target": {"databases": [{"server_uuid": UUID, "database": "protected_target"}]}}})
}

fn prepared(request: &Value, generation: &Value, descriptor: &Value) -> Value {
    let databases = request["target"]["databases"].as_array().unwrap();
    let intents = databases.iter().map(|db| json!({"key": db["key"], "server_uuid": db["server_uuid"], "database": db["database"]})).collect::<Vec<_>>();
    let databases = databases.iter().map(|db| json!({"server_uuid": db["server_uuid"], "database": db["database"], "exists": false, "empty_checked": false})).collect::<Vec<_>>();
    let objects = ["uploads", "avatar", "exports", "imports", "config-packages"]
        .into_iter()
        .map(|bucket| (bucket.into(), json!({"keys": []})))
        .collect::<serde_json::Map<String, Value>>();
    json!({"format_version": 1, "status": "fresh_creation_prepared", "prepared_at": "2026-01-01T00:00:00Z", "request": descriptor, "request_sha256": digest::plan_hash(request).unwrap(), "generation": generation, "absence": {"databases": databases, "objects": objects, "redis": {"keys": [], "owner": null, "sentinel": null}}, "fresh_target_initialized": false, "controlled_generation_only": true, "external_writers_discovered": false, "create_intents": intents})
}

fn database_name(side: &str, key: &str) -> String {
    format!("{side}_{}", key.replace('-', "_"))
}
fn write(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}
fn bound(path: &Path) -> Value {
    manifest::binding(path, &digest::file(path).unwrap()).unwrap()
}
fn tool(path: &Path) -> Value {
    json!({"path": path, "sha256": digest::file(path).unwrap().sha256})
}
fn review_binding(path: &Path, value: &Value) -> Value {
    let mut result = bound(path);
    result["canonical_sha256"] = json!(digest::plan_hash(value).unwrap());
    result
}

#[test]
fn request_rejects_review_digest_and_readiness_drift() {
    let mut fixture = Fixture::new();
    fixture.validate().unwrap();
    fixture.request["review"]["canonical_sha256"] = json!("0".repeat(64));
    assert!(fixture.validate().is_err());
    fixture.seal_review();
    fixture.review["ready_for_execution"] = json!(false);
    fixture.seal_review();
    assert!(fixture.validate().is_err());
    fixture.review["ready_for_execution"] = json!(true);
    fixture.seal_review();
    fs::write(fixture.local.join("review.json"), b"{}").unwrap();
    assert!(fixture.validate().is_err());
}

#[test]
fn request_requires_exact_keys_modes_and_three_sides() {
    let mut fixture = Fixture::new();
    let original = fixture.request.clone();
    for (path, value) in [
        ("/target/databases/1/key", json!("unknown")),
        ("/target/databases/1/mode", json!("dedicated")),
        ("/target/databases/2/kind", json!("combined")),
        ("/target/databases/3/key", json!("dedicated-a")),
        ("/target/databases/1/database", json!("other")),
        ("/target/databases/1/server_uuid", json!("other")),
    ] {
        *fixture.request.pointer_mut(path).unwrap() = value;
        assert!(fixture.validate().is_err(), "{path}");
        fixture.request = original.clone();
    }
    fixture.review["scopes"]
        .as_object_mut()
        .unwrap()
        .remove("candidate");
    fixture.seal_review();
    assert!(fixture.validate().is_err());
}

#[test]
fn request_rejects_case_folded_cross_side_and_protected_collisions() {
    let mut fixture = Fixture::new();
    fixture.review["scopes"]["base"]["databases"][0]["database"] = json!("SEED_SHARED_CONTROL");
    fixture.seal_review();
    assert!(fixture.validate().is_err());
    fixture.review["scopes"]["base"]["databases"][0]["database"] = json!("base_shared_control");
    fixture.review["reference"]["protected_target"]["databases"][0]["database"] =
        json!("CANDIDATE_SHARED");
    fixture.seal_review();
    assert!(fixture.validate().is_err());
}

#[test]
fn request_rejects_unreviewed_tool_and_defaults_changes() {
    let mut fixture = Fixture::new();
    fs::write(fixture.local.join("replacement"), b"replacement").unwrap();
    fixture.request["tools"]["mysql"] = tool(&fixture.local.join("replacement"));
    assert!(fixture.validate().is_err());
    fixture.request["tools"]["mysql"] = fixture.review["tools"]["mysql"].clone();
    fs::write(fixture.local.join("mysql.cnf"), b"changed").unwrap();
    assert!(fixture.validate().is_err());
}

#[test]
fn request_requires_three_true_exclusive_flags_and_exact_sentinel() {
    let mut fixture = Fixture::new();
    let original = fixture.request.clone();
    for key in request::EXCLUSIVE {
        for value in [json!(false), json!("true"), Value::Null] {
            fixture.request["reset"]["legacy_ownership"][key] = value;
            assert!(fixture.validate().is_err());
            fixture.request = original.clone();
        }
    }
    fixture.request["reset"]["sentinel_value"] = json!("wrong-request");
    assert!(fixture.validate().is_err());
}

#[test]
fn request_name_uses_current_shared_python_equivalent_rule() {
    let mut fixture = Fixture::new();
    for name in [
        "a",
        "ab",
        "abc",
        &"a".repeat(48),
        &"a".repeat(49),
        "a_b",
        "Aaa",
    ] {
        fixture.request["id"] = json!(name);
        fixture.request["reset"]["sentinel_value"] = json!(format!("devex-fresh:{name}"));
        assert_eq!(
            fixture.validate().is_ok(),
            native_input::name(name).is_ok(),
            "{name}"
        );
    }
}

struct Snapshot(Value);
impl request::SourceObserver for Snapshot {
    fn snapshot(&self, _: &Path) -> Result<Value> {
        Ok(self.0.clone())
    }
}

#[test]
fn request_fixture_requires_ready_receipt_exact_root_and_observed_snapshot() {
    let mut fixture = Fixture::new();
    let root = fixture.local.join("business");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("Cargo.toml"), "fixture").unwrap();
    fs::write(root.join(".git"), "gitdir: fixture").unwrap();
    let snapshot = json!({"head": "a".repeat(40), "patch_sha256": "b".repeat(64), "files": []});
    let receipt = fixture.local.join("fixture.json");
    write(
        &receipt,
        &json!({"format_version": 1, "fixture": "business", "status": "ready", "paths": {"backend": root}, "generated": {"backend": snapshot}}),
    );
    fixture.request["execution_backend"] = json!({"fixture": bound(&receipt), "path": root});
    request::validate_with(&fixture.backend, &fixture.request, &Snapshot(snapshot)).unwrap();
    assert!(
        request::validate_with(
            &fixture.backend,
            &fixture.request,
            &Snapshot(json!({"changed": true}))
        )
        .is_err()
    );
    let mut bad = crate::recovery::clone::native::json::read(&receipt).unwrap();
    bad["status"] = json!("pending");
    write(&receipt, &bad);
    fixture.request["execution_backend"]["fixture"] = bound(&receipt);
    assert!(
        request::validate_with(
            &fixture.backend,
            &fixture.request,
            &Snapshot(bad["generated"]["backend"].clone())
        )
        .is_err()
    );
}

#[test]
fn configuration_binds_secrets_physical_connections_and_file_overrides() {
    let mut fixture = Fixture::new();
    let original_env = fixture.environment.clone();
    let binding = fixture.validate().unwrap();
    request_configuration::validate(
        &fixture.backend,
        &fixture.request,
        &binding,
        &fixture.environment,
    )
    .unwrap();
    for (key, value) in [
        ("APP_TENANT_PASSWORD", "wrong-secret"),
        ("APP_DATABASE_HOST_FILE", "unbound.txt"),
        ("APP_RESET_LEGACY_MYSQL_EXCLUSIVE", "false"),
        ("APP_REDIS_PORT", "26380"),
    ] {
        fixture.environment.insert(key.into(), value.into());
        fixture.seal_configuration();
        let error = request_configuration::validate(
            &fixture.backend,
            &fixture.request,
            &binding,
            &fixture.environment,
        )
        .unwrap_err()
        .to_string();
        assert!(!error.contains("wrong-secret"));
        fixture.environment = original_env.clone();
    }
    fixture.seal_configuration();
    let before = fixture.generation();
    fixture.environment.insert(
        "APP_OBJECT_STORAGE_SECRET_KEY".into(),
        "changed-storage-secret".into(),
    );
    fixture.seal_configuration();
    let changed = fixture.generation();
    assert_ne!(changed["configuration"], before["configuration"]);
    assert_ne!(changed["request_sha256"], before["request_sha256"]);
    assert!(!changed.to_string().contains("changed-storage-secret"));
    fixture.environment = original_env;
    fixture.seal_configuration();
    fixture
        .environment
        .insert("RYFRAME_RESET_USER_PASSWORD".into(), "changed-seed".into());
    let changed = request_configuration::validate(
        &fixture.backend,
        &fixture.request,
        &binding,
        &fixture.environment,
    )
    .unwrap();
    assert_ne!(
        changed["seed_credentials_sha256"],
        before["seed_credentials_sha256"]
    );
}

#[test]
fn configuration_detects_file_drift_and_missing_seed_credentials() {
    let mut fixture = Fixture::new();
    let binding = fixture.validate().unwrap();
    fs::write(fixture.backend.join("config/extra.toml"), "x = 1\n").unwrap();
    assert!(
        request_configuration::validate(
            &fixture.backend,
            &fixture.request,
            &binding,
            &fixture.environment
        )
        .is_err()
    );
    fixture.seal_configuration();
    fixture.environment.remove("RYFRAME_RESET_ADMIN_PASSWORD");
    assert!(
        request_configuration::validate(
            &fixture.backend,
            &fixture.request,
            &binding,
            &fixture.environment
        )
        .is_err()
    );
}

#[test]
fn mysql_defaults_rejects_unknown_options_and_supported_quotes_match() {
    let fixture = Fixture::new();
    let path = fixture.local.join("mysql.cnf");
    let mut db = fixture.request["target"]["databases"][0].clone();
    fs::write(&path, "[client]\nhost=127.0.0.1\nport=3306\nuser=\"fixture\"\npassword=\"fixture-secret\"\nssl-mode=DISABLED\n").unwrap();
    db["defaults_sha256"] = json!(digest::file(&path).unwrap().sha256);
    assert_eq!(
        request_physical::defaults_connection(&fixture.backend, &db).unwrap()["username"],
        json!("fixture")
    );
    for extra in ["socket=other\n", "password=duplicate\n", "# comment\n"] {
        let body = format!(
            "[client]\nhost=127.0.0.1\nport=3306\nuser=fixture\npassword=fixture-secret\nssl-mode=DISABLED\n{extra}"
        );
        fs::write(&path, body).unwrap();
        db["defaults_sha256"] = json!(digest::file(&path).unwrap().sha256);
        assert!(request_physical::defaults_connection(&fixture.backend, &db).is_err());
    }
}

#[test]
fn python_configuration_digest_preserves_default_spacing_and_ascii_escapes() {
    let value = json!({"environment": {"APP_NAME": "中文😀\u{7f}"}, "config_directory": "D:\\配置", "files": {"app.toml": "a".repeat(64)}});
    assert_eq!(
        request_configuration::python_configuration_hash(&value).unwrap(),
        "7713692a615dfac40739f68b60dd606835da2dc88f0136d23091d6b7b72d01ef"
    );
}

#[test]
fn preflight_accepts_no_attempt_without_creating_output_and_load_requires_attempt() {
    let fixture = Fixture::new();
    let (run, _) = fixture.publish();
    let stage = StageRequest {
        backend: &fixture.backend,
        run_dir: &run,
        stage: "target-verify",
        mode: "run",
        attempt: None,
    };
    validate_inputs(&fixture.backend, &stage).unwrap();
    assert!(TargetContext::load(&fixture.backend, &stage).is_err());
    assert!(!run.join("v0000").exists());
    let stage = StageRequest {
        attempt: Some(1),
        ..stage
    };
    TargetContext::load(&fixture.backend, &stage).unwrap();
    assert!(!run.join("v0001").exists());
}

#[test]
fn prepare_evidence_rejects_changed_request_generation_absence_and_started_digest() {
    let fixture = Fixture::new();
    let (_, target) = fixture.publish();
    let read = |name: &str| crate::recovery::clone::native::json::read(&target.join(name)).unwrap();
    let original = read("prepare.json");
    let initialized = read("initialized.json");
    request_evidence::validate_prepare(
        &fixture.backend,
        &target,
        &original,
        &initialized,
        &fixture.request,
    )
    .unwrap();
    for (path, value) in [
        ("/request_sha256", json!("0".repeat(64))),
        ("/generation/selected/scope_id", json!("other")),
        ("/absence/redis/keys", json!(["unowned"])),
        ("/fresh_target_initialized", json!(true)),
    ] {
        let mut bad = original.clone();
        *bad.pointer_mut(path).unwrap() = value;
        assert!(
            request_evidence::validate_prepare(
                &fixture.backend,
                &target,
                &bad,
                &initialized,
                &fixture.request
            )
            .is_err(),
            "{path}"
        );
    }
    write(
        &target.join("initialize.started.json"),
        &json!({"at": "2026-01-01T00:01:00Z", "generation_sha256": "0".repeat(64)}),
    );
    assert!(
        request_evidence::validate_prepare(
            &fixture.backend,
            &target,
            &original,
            &initialized,
            &fixture.request
        )
        .is_err()
    );
}

#[test]
fn initialized_requires_explicit_never_started_and_no_external_writers() {
    let fixture = Fixture::new();
    let (_, target) = fixture.publish();
    let original =
        crate::recovery::clone::native::json::read(&target.join("initialized.json")).unwrap();
    for (field, value) in [
        ("controlled_generation_never_started", json!(false)),
        ("controlled_generation_never_started", json!("true")),
        ("external_writers_discovered", json!(true)),
        ("external_writers_discovered", Value::Null),
    ] {
        let mut bad = original.clone();
        bad[field] = value;
        assert!(super::validate_initialized(&bad).is_err());
    }
}
