use super::*;
use crate::recovery::{
    clone::native::target_verify::input::request::{self, SourceObserver, SystemSourceObserver},
    fixture_control::{
        environment::protocol::tests as fixtures, source_pair::execution::tests::Fixture,
    },
    inputs::document,
};
use serde_json::json;
use std::fs;

fn selected(fixture: &Fixture) -> Value {
    json!({"path":fixture.output.join("backend"),"execution_source":fixture.binding()})
}

#[test]
fn published_target_consumers_bind_the_actual_clean_commit_and_reject_drift() {
    let fixture = Fixture::new();
    let publication = fixture.publication();
    let root = fixture.output.join("backend");
    let declared = selected(&fixture);
    for coordinator in [&fixture.backend, &root] {
        let expected = execution_backend(coordinator, Some(&declared), None).unwrap();
        let actual =
            request::execution_backend(coordinator, Some(&declared), &SystemSourceObserver)
                .unwrap();
        assert_eq!(expected, actual);
        assert_eq!(actual.1["kind"], "published-business-fixture");
        assert_eq!(
            actual.1["fixture"],
            publication["original"]["fixture_receipt"]
        );
        assert_eq!(
            actual.1["source"]["head"],
            publication["published"]["backend"]["head"]
        );
        assert_ne!(
            actual.1["source"]["head"],
            publication["original"]["source_pair"]["sources"]["backend"]["head"]
        );
        assert!(actual.1["source"].get("clean").is_none());
    }
    let source = root.join("src/lib.rs");
    let bytes = fs::read(&source).unwrap();
    fs::write(&source, b"dirty published source").unwrap();
    assert!(execution_backend(&root, Some(&declared), None).is_err());
    assert!(
        std::process::Command::new("git")
            .args(["add", "src/lib.rs"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    fs::write(&source, bytes).unwrap();
    assert!(execution_backend(&root, Some(&declared), None).is_err());
    assert!(
        std::process::Command::new("git")
            .args(["reset", "--quiet", "HEAD", "--", "src/lib.rs"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    execution_backend(&root, Some(&declared), None).unwrap();
    let original = fixture.backend.join("src/lib.rs");
    let bytes = fs::read(&original).unwrap();
    fs::write(&original, b"original drift").unwrap();
    assert!(request::execution_backend(&root, Some(&declared), &SystemSourceObserver).is_err());
    fs::write(original, bytes).unwrap();
    request::execution_backend(&root, Some(&declared), &SystemSourceObserver).unwrap();
}

#[test]
fn published_selection_rejects_mixed_roots_manifest_changes_and_observer_mismatch() {
    let fixture = Fixture::new();
    fixture.publication();
    let declared = selected(&fixture);
    let root = fixture.output.join("backend");
    assert!(execution_backend(&fixture.frontend, Some(&declared), None).is_err());
    let mut mixed = declared.clone();
    mixed["fixture"] = json!({});
    let mut missing = declared.clone();
    missing.as_object_mut().unwrap().remove("execution_source");
    let mut wrong = declared.clone();
    wrong["execution_source"]["sha256"] = json!("0".repeat(64));
    let mut other = declared.clone();
    other["path"] = json!(fixture.frontend);
    for invalid in [mixed, missing, wrong, other] {
        assert!(execution_backend(&root, Some(&invalid), None).is_err());
        assert!(request::execution_backend(&root, Some(&invalid), &SystemSourceObserver).is_err());
    }
    struct Wrong;
    impl SourceObserver for Wrong {
        fn snapshot(&self, _: &Path) -> Result<Value> {
            Ok(json!({}))
        }
    }
    assert!(request::execution_backend(&root, Some(&declared), &Wrong).is_err());
    let manifest = fixture.output.join("execution-source.json");
    let bytes = fs::read(&manifest).unwrap();
    fs::write(&manifest, b"{}").unwrap();
    assert!(execution_backend(&root, Some(&declared), None).is_err());
    let mut changed: Value = serde_json::from_slice(&bytes).unwrap();
    changed["original"]["source_pair"]["sources"]["backend"]["head"] = json!("0".repeat(40));
    fs::write(&manifest, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(execution_backend(&root, Some(&selected(&fixture)), None).is_err());
    fs::write(manifest, bytes).unwrap();
    execution_backend(&root, Some(&declared), None).unwrap();
}

fn measurement_inputs(backend: &Path) -> (Value, Value, Value) {
    let local = backend.join(".local-tests");
    fs::create_dir(&local).unwrap();
    let config = backend.join("config");
    fs::create_dir(&config).unwrap();
    let defaults = local.join("mysql.cnf");
    let mysql = local.join("mysql");
    fs::write(&defaults, b"[client]\n").unwrap();
    fs::write(&mysql, b"fixture tool").unwrap();
    let maintenance = write_document(backend, "maintenance.json", &json!({}));

    let mut comparison = fixtures::review(backend);
    comparison["tools"]["mysql"] = json!({"path":mysql,"sha256":file_sha256(&mysql).unwrap()});
    for role in ["mysqldump", "rustfs"] {
        comparison["tools"][role]["sha256"] = json!("a".repeat(64));
    }
    let candidate = &mut comparison["scopes"]["candidate"];
    let root = local.join("measurement");
    candidate["runtime_dir"] = json!(root.join("runtime-candidate"));
    candidate["identity_ledger"] = json!(root.join("identities-candidate"));
    candidate["objects"]["endpoint"] = json!("http://127.0.0.1:29200");
    candidate["redis"]["namespace"] = json!("ryframe:{fixture-candidate}:");
    candidate["redis"]["ownership_key"] = json!("ryframe:{fixture-candidate}:.ryframe-owner");
    candidate["redis"]["ownership_value"] = json!("ryframe-owner:v1:fixture-candidate:redis");
    let databases = candidate["databases"].as_array_mut().unwrap();
    for database in databases {
        database["connection_file"] = json!(defaults);
    }
    let defaults_digest = file_sha256(&defaults).unwrap();
    let databases = candidate["databases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|database| {
            let (kind, mode) = mode_for_key(database["key"].as_str().unwrap()).unwrap();
            json!({"key":database["key"],"kind":kind,"mode":mode,"database":database["database"],
            "server_uuid":database["expected_server_uuid"],"defaults_file":defaults,
            "defaults_sha256":defaults_digest})
        })
        .collect::<Vec<_>>();
    let mut measurement = comparison.clone();
    measurement["kind"] = json!("review-only-b1-runtime-measurement-with-readonly-preflight");
    measurement["scopes"] = json!({"candidate": comparison["scopes"]["candidate"]});
    measurement["future_root"] = json!(root);
    measurement["services"]["rustfs"]["data_dir"] = json!(root.join("rustfs"));
    measurement["services"]["redis"] =
        json!({"directory":root.join("redis"),"endpoint":"127.0.0.1:16390"});
    let tools = measurement["tools"].as_object_mut().unwrap();
    for role in ["wsl", "redis_python"] {
        tools.remove(role);
    }
    tools.insert("redis_server".to_owned(), json!({
        "platform":"windows","path":"D:/tools/redis-server.exe","sha256":"a".repeat(64),
        "version":"Redis server v=7.4.11 sha=00000000:0 malloc=libc bits=64 build=5bd2587910691cb8",
        "dlls":{"msys-2.0.dll":"a".repeat(64)}
    }));
    let request = json!({
        "format_version":1,"kind":"devex-clone-fresh-target","id":"fixture-request","side":"candidate",
        "target":{"scope_id":"fixture-candidate","s3":{"endpoint":"http://127.0.0.1:29200",
            "region":"us-east-1","access_key":"fixture-key","secret_key":"fixture-secret"},"databases":databases},
        "maintenance_build":document::binding(&maintenance),"configuration_sha256":"b".repeat(64),
        "config_dir":config,"tools":{"mysql":measurement["tools"]["mysql"]},
        "storage":{"rustfs":{},"redis":{}},"reset":{
            "credential_version":"fixture",
            "legacy_ownership":{"mysql_exclusive":true,"redis_exclusive":true,"object_storage_exclusive":true},
            "sentinel_key":"ryframe:devex-fresh:fixture-candidate:sentinel","sentinel_value":"devex-fresh:fixture-request"}
    });
    (comparison, measurement, request)
}

fn write_document(backend: &Path, file: &str, value: &Value) -> Document {
    let path = backend.join(".local-tests").join(file);
    fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    document::read(&path, backend, "目标测试输入").unwrap()
}

fn bind_review(backend: &Path, review: &Value, request: &Value) -> Value {
    let review_document = write_document(backend, "review.json", review);
    let mut request = request.clone();
    request["review"] = document::binding(&review_document);
    request["review"]["canonical_sha256"] = json!(document::hash(review).unwrap());
    request
}

fn measurement_directory() -> tempfile::TempDir {
    let local = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    tempfile::tempdir_in(local).unwrap()
}

#[test]
fn independent_measurement_supplies_only_the_registered_candidate() {
    let directory = measurement_directory();
    let backend = directory.path();
    let (comparison, measurement, request) = measurement_inputs(backend);
    let bound = bind_review(backend, &measurement, &request);
    let candidate = write_document(backend, "target.json", &bound);
    let target = validate_b1_measurement(backend, candidate.clone(), None).unwrap();
    assert_eq!(target.selected, measurement["scopes"]["candidate"]);
    assert_eq!(target.execution, backend);
    assert_eq!(target.document.raw, candidate.raw);
    assert!(validate(backend, candidate, "candidate").is_err());

    let bound = bind_review(backend, &comparison, &request);
    let candidate = write_document(backend, "target.json", &bound);
    assert!(validate(backend, candidate.clone(), "candidate").is_ok());
    assert!(validate_b1_measurement(backend, candidate.clone(), None).is_err());
    let review_path = backend.join(".local-tests/review.json");
    let mut raw = fs::read(&review_path).unwrap();
    raw.push(b'\n');
    fs::write(review_path, raw).unwrap();
    assert!(validate(backend, candidate, "candidate").is_err());
}

#[test]
fn independent_measurement_rejects_role_scope_database_marker_and_digest_drift() {
    let directory = measurement_directory();
    let backend = directory.path();
    let (comparison, measurement, request) = measurement_inputs(backend);
    let bound = bind_review(backend, &measurement, &request);
    let mut seed = bound.clone();
    seed["side"] = json!("seed");
    let mut base = bound.clone();
    base["side"] = json!("base");
    let mut scope = bound.clone();
    scope["target"]["scope_id"] = json!("fixture-other");
    let mut database = bound.clone();
    database["target"]["databases"][0]["database"] = json!("different_database");
    let mut sentinel = bound.clone();
    sentinel["reset"]["sentinel_value"] = json!("devex-fresh:another-request");
    let mut digest = bound.clone();
    digest["review"]["canonical_sha256"] = json!("0".repeat(64));
    for invalid in [seed, base, scope, database, sentinel, digest] {
        let document = write_document(backend, "target.json", &invalid);
        assert!(validate_b1_measurement(backend, document, None).is_err());
    }

    let mut extra = measurement.clone();
    extra["scopes"]["base"] = comparison["scopes"]["base"].clone();
    let mut duplicate = measurement.clone();
    let databases = duplicate["scopes"]["candidate"]["databases"]
        .as_array_mut()
        .unwrap();
    databases.push(databases[0].clone());
    let mut foreign = measurement.clone();
    foreign["scopes"]["candidate"]["backend_dir"] = json!(backend.join("other"));
    for invalid in [extra, duplicate, foreign] {
        let bound = bind_review(backend, &invalid, &request);
        let document = write_document(backend, "target.json", &bound);
        assert!(validate_b1_measurement(backend, document, None).is_err());
    }
}
