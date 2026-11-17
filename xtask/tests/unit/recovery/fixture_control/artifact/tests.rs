use super::*;
use crate::recovery::clone::native::target_verify::input::request_configuration::python_configuration_hash;
use std::{cell::RefCell, collections::BTreeMap, fs};

struct Database {
    record: Value,
    remaining: u64,
}
impl EvidenceDatabase for Database {
    fn record(&mut self, _: i64) -> Result<Value> {
        Ok(self.record.clone())
    }
    fn remaining(&mut self, _: i64, _: i64) -> Result<u64> {
        Ok(self.remaining)
    }
    fn name(&self) -> &str {
        "owned_test"
    }
}
struct Objects(RefCell<BTreeMap<String, Vec<u8>>>);
impl EvidenceObjects for Objects {
    fn bytes(&self, key: &str) -> Result<Option<Vec<u8>>> {
        objects::key(key)?;
        Ok(self.0.borrow().get(key).cloned())
    }
}
fn fixture() -> (tempfile::TempDir, FixtureArtifactOptions, Database, Objects) {
    let directory = tempfile::tempdir().unwrap();
    let options = FixtureArtifactOptions {
        runtime_dir: directory.path().into(),
        receipt: directory.path().join("receipt.json"),
        job_id: 123,
    };
    let data = b"actual export data".to_vec();
    let database = Database {
        record: json!({"job_id":"123","file_id":"456","bucket":"exports","key":"result.xlsx","bytes":data.len(),"sha256":crate::devex::runtime_native::artifacts::sha(&data)}),
        remaining: 0,
    };
    let objects = Objects(RefCell::new(BTreeMap::from([
        ("result.xlsx".into(), data),
        (
            ".ryframe-owner".into(),
            b"ryframe-owner:v1:artifact-test:object-storage:exports".to_vec(),
        ),
    ])));
    (directory, options, database, objects)
}
fn inspect_fixture(
    options: &FixtureArtifactOptions,
    snapshot: bool,
    database: &mut Database,
    objects: &Objects,
) -> Result<Value> {
    inspect(
        options,
        snapshot,
        "artifact-test",
        "server-uuid",
        database,
        objects,
    )
}

#[test]
fn artifact_requires_both_metadata_rows_and_exact_object_to_disappear() {
    let (_directory, options, mut database, objects) = fixture();
    let receipt = inspect_fixture(&options, true, &mut database, &objects).unwrap();
    super::super::write_new(&options.receipt, &receipt).unwrap();
    assert_eq!(
        inspect_fixture(&options, false, &mut database, &objects).unwrap()["state"],
        "pending"
    );
    objects.0.borrow_mut().remove("result.xlsx");
    assert_eq!(
        inspect_fixture(&options, false, &mut database, &objects).unwrap()["state"],
        "deleted"
    );
    database.remaining = 1;
    assert_eq!(
        inspect_fixture(&options, false, &mut database, &objects).unwrap()["state"],
        "pending"
    );
}

#[test]
fn artifact_rejects_corrupt_content_owner_and_cross_resource_receipt() {
    let (_directory, options, mut database, objects) = fixture();
    objects
        .0
        .borrow_mut()
        .insert("result.xlsx".into(), b"corrupt".to_vec());
    assert!(
        inspect_fixture(&options, true, &mut database, &objects)
            .unwrap_err()
            .to_string()
            .contains("摘要")
    );
    for field in [
        "scope_id",
        "server_uuid",
        "database",
        "job_id",
        "bucket",
        "file_id",
        "key",
        "sha256",
        "bytes",
    ] {
        let mut receipt = database.record.clone();
        receipt["scope_id"] = json!("artifact-test");
        receipt["server_uuid"] = json!("server-uuid");
        receipt["database"] = json!("owned_test");
        receipt[field] = if field == "key" {
            json!("../invalid")
        } else {
            json!("invalid")
        };
        fs::write(&options.receipt, serde_json::to_vec(&receipt).unwrap()).unwrap();
        assert!(
            inspect_fixture(&options, false, &mut database, &objects).is_err(),
            "{field}"
        );
    }
    objects
        .0
        .borrow_mut()
        .insert(".ryframe-owner".into(), b"other".to_vec());
    assert!(
        inspect_fixture(&options, true, &mut database, &objects)
            .unwrap_err()
            .to_string()
            .contains("ownership")
    );
}

#[test]
fn artifact_paths_ids_and_create_only_receipts_fail_closed() {
    for key in [
        "../outside",
        "/absolute",
        "x/../y",
        "x\\y",
        "x:stream",
        "x//y",
        "",
    ] {
        assert!(objects::key(key).is_err(), "{key}");
    }
    for id in ["0", "01", "1 OR 1=1", "-1", "9223372036854775808"] {
        assert!(identifier(id).is_err());
    }
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join(".local-tests")).unwrap();
    let runtime = directory.path().join(".local-tests/runtime");
    fs::create_dir(&runtime).unwrap();
    let mut options = FixtureArtifactOptions {
        runtime_dir: runtime.clone(),
        receipt: runtime.join("receipt.json"),
        job_id: 123,
    };
    validate(&options, directory.path(), true).unwrap();
    fs::write(&options.receipt, b"{}").unwrap();
    assert!(validate(&options, directory.path(), true).is_err());
    validate(&options, directory.path(), false).unwrap();
    options.receipt = directory.path().join("outside.json");
    assert!(validate(&options, directory.path(), true).is_err());
}

#[test]
fn artifact_s3_remote_endpoint_is_rejected_before_request() {
    let directory = tempfile::tempdir().unwrap();
    let table: toml::Table =
        toml::from_str("[object_storage]\nbackend='s3'\nendpoint='https://example.test'\n")
            .unwrap();
    assert!(
        objects::Objects::new(directory.path(), directory.path(), &table, "artifact-test").is_err()
    );
}

#[test]
fn artifact_python_json_digest_sorts_keys_and_preserves_unicode_and_spacing() {
    assert_eq!(
        python_configuration_hash(&json!({"b":1,"a":["中文", "😀"]})).unwrap(),
        "ae21fbaca5c9727af78ce6399a82f784d2558d3f4f4b7cc5987ebe340d4289d7"
    );
}

#[test]
fn artifact_local_object_reads_exact_scope_and_enforces_missing_state() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().join("objects");
    let scope = base.join("exports/artifact-test");
    fs::create_dir_all(&scope).unwrap();
    let table: toml::Table = toml::from_str(&format!(
        "[object_storage]\nbackend='local'\nlocal_base_dir='{}'\n",
        base.to_string_lossy().replace('\\', "/")
    ))
    .unwrap();
    let objects =
        objects::Objects::new(directory.path(), directory.path(), &table, "artifact-test").unwrap();
    assert_eq!(objects.bytes("result.xlsx").unwrap(), None);
    fs::write(scope.join("result.xlsx"), b"data").unwrap();
    assert_eq!(
        objects.bytes("result.xlsx").unwrap(),
        Some(b"data".to_vec())
    );
}

fn runtime_fixture(root: &Path) -> (std::path::PathBuf, Value) {
    let directory = root.join("runtime");
    let api = root.join("config");
    let worker = root.join("worker-config");
    for path in [&directory, &api, &worker] {
        fs::create_dir_all(path).unwrap();
    }
    for (path, id) in [(&api, 3), (&worker, 2)] {
        fs::write(path.join("app.test.toml"), format!("scope_id='artifact-test'\nsnowflake_worker_id={id}\n[jobs]\nmode='external'\nhealth_host='127.0.0.1'\nhealth_port=18081\n")).unwrap();
    }
    let mut binaries = serde_json::Map::new();
    let mut artifacts = serde_json::Map::new();
    for (name, role) in [
        ("ryframe", "api"),
        ("ryframe-worker", "worker"),
        ("ryframe-reset", "reset"),
        ("ryframe-migrate", "migrate"),
    ] {
        let path = directory.join(format!("{name}.exe"));
        fs::write(&path, name).unwrap();
        binaries.insert(name.into(), json!(path));
        if matches!(role, "api" | "worker") {
            artifacts.insert(role.into(), json!({"path":path,"sha256":crate::devex::runtime_native::artifacts::sha(name.as_bytes())}));
        }
    }
    fs::write(
        directory.join("binaries.json"),
        serde_json::to_vec(&binaries).unwrap(),
    )
    .unwrap();
    let api_digest = runtime::configuration_digest(&api).unwrap();
    let worker_digest = runtime::configuration_digest(&worker).unwrap();
    let receipt = json!({"format_version":1,"backend_root":root,"scope_id":"artifact-test","api_config_dir":api,"worker_config_dir":worker,"api_configuration_sha256":api_digest,"worker_configuration_sha256":worker_digest,"configuration_sha256":python_configuration_hash(&json!([api_digest,worker_digest])).unwrap(),"worker_ready_url":"http://127.0.0.1:18081/readyz","artifacts":artifacts});
    fs::write(
        directory.join("runtime.json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    (directory, receipt)
}

#[test]
fn artifact_runtime_binds_files_scope_instances_and_all_binaries() {
    let root = tempfile::tempdir().unwrap();
    let (directory, receipt) = runtime_fixture(root.path());
    let equivalent_root = root.path().to_string_lossy().replace('\\', "/");
    assert_eq!(
        runtime::verify(Path::new(&equivalent_root), &directory)
            .unwrap()
            .receipt,
        receipt
    );
    let manifest = directory.join("binaries.json");
    let original_manifest = fs::read(&manifest).unwrap();
    let manifest_value: Value = serde_json::from_slice(&original_manifest).unwrap();
    let mut missing_role = manifest_value.clone();
    assert!(
        missing_role
            .as_object_mut()
            .unwrap()
            .remove("ryframe-reset")
            .is_some()
    );
    let mut relative_path = manifest_value.clone();
    relative_path["ryframe-reset"] = json!("relative-reset.exe");
    let mut missing_file = manifest_value;
    missing_file["ryframe-reset"] = json!(directory.join("missing-reset.exe"));

    let runtime_receipt = directory.join("runtime.json");
    let original_receipt = fs::read(&runtime_receipt).unwrap();
    for (changed, reason) in [
        (missing_role, "全栈二进制清单"),
        (relative_path, "路径必须是绝对路径"),
        (missing_file, "输入路径不存在"),
    ] {
        let changed_bytes = serde_json::to_vec(&changed).unwrap();
        fs::write(&manifest, &changed_bytes).unwrap();
        let error = runtime::verify(root.path(), &directory).err().unwrap();
        assert!(error.to_string().contains(reason), "{reason}: {error}");
        assert_eq!(fs::read(&manifest).unwrap(), changed_bytes);
        assert_eq!(fs::read(&runtime_receipt).unwrap(), original_receipt);
    }
    fs::write(&manifest, original_manifest).unwrap();

    let config = root.path().join("config/app.test.toml");
    let original = fs::read(&config).unwrap();
    fs::write(&config, b"scope_id='other-scope'").unwrap();
    assert!(
        runtime::verify(root.path(), &directory)
            .err()
            .unwrap()
            .to_string()
            .contains("摘要")
    );
    fs::write(&config, original).unwrap();
    fs::write(directory.join("ryframe.exe"), b"changed").unwrap();
    assert!(runtime::verify(root.path(), &directory).is_err());
    assert_eq!(
        runtime::verify_configuration(root.path(), &directory)
            .unwrap()
            .receipt,
        receipt
    );
    fs::write(directory.join("ryframe.exe"), b"ryframe").unwrap();
    fs::remove_file(directory.join("ryframe-reset.exe")).unwrap();
    assert!(runtime::verify(root.path(), &directory).is_err());
    assert!(runtime::verify_configuration(root.path(), &directory).is_ok());
}

#[test]
fn artifact_runtime_rejects_duplicate_json_and_unexpected_evidence() {
    let root = tempfile::tempdir().unwrap();
    let (directory, receipt) = runtime_fixture(root.path());
    fs::write(directory.join("runtime-evidence.json"), b"{}").unwrap();
    assert!(
        runtime::verify(root.path(), &directory)
            .err()
            .unwrap()
            .to_string()
            .contains("意外")
    );
    fs::remove_file(directory.join("runtime-evidence.json")).unwrap();
    let encoded = serde_json::to_string(&receipt).unwrap();
    fs::write(
        directory.join("runtime.json"),
        encoded.replacen('{', "{\"format_version\":1,", 1),
    )
    .unwrap();
    assert!(runtime::verify(root.path(), &directory).is_err());
}

#[test]
fn configuration_only_contract_keeps_both_directories_worker_drift_and_artifact_shape_strict() {
    let root = tempfile::tempdir().unwrap();
    let (directory, original) = runtime_fixture(root.path());
    let alternate = root.path().join("isolated");
    fs::create_dir(&alternate).unwrap();
    for name in ["config", "worker-config"] {
        fs::rename(root.path().join(name), alternate.join(name)).unwrap();
    }
    let mut receipt = original;
    receipt["api_config_dir"] = json!(alternate.join("config"));
    receipt["worker_config_dir"] = json!(alternate.join("worker-config"));
    let path = directory.join("runtime.json");
    fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    assert!(runtime::verify_configuration(root.path(), &directory).is_err());
    let api_digest = runtime::configuration_digest(&alternate.join("config")).unwrap();
    let worker_digest = runtime::configuration_digest(&alternate.join("worker-config")).unwrap();
    receipt["api_configuration_sha256"] = json!(api_digest);
    receipt["worker_configuration_sha256"] = json!(worker_digest);
    receipt["configuration_sha256"] =
        json!(python_configuration_hash(&json!([api_digest, worker_digest])).unwrap());
    fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    runtime::verify_configuration(root.path(), &directory).unwrap();
    let worker = alternate.join("worker-config/app.test.toml");
    let bytes = fs::read(&worker).unwrap();
    fs::write(&worker, b"scope_id='changed'").unwrap();
    assert!(runtime::verify_configuration(root.path(), &directory).is_err());
    fs::write(&worker, &bytes).unwrap();
    for (pointer, value) in [
        (
            "/worker_config_dir",
            json!(root.path().join("worker-config")),
        ),
        ("/worker_ready_url", json!("http://127.0.0.1:18082/readyz")),
        ("/artifacts/api/sha256", json!("unknown")),
        ("/artifacts/worker/path", json!("relative.exe")),
        (
            "/artifacts/api",
            json!({"path":directory.join("ryframe.exe"),"sha256":"a".repeat(64),"extra":true}),
        ),
    ] {
        let mut changed = receipt.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(
            runtime::verify_configuration(root.path(), &directory).is_err(),
            "{pointer}"
        );
    }
    // 文件摘要同步变化仍不能掩盖 Worker 与 API 的合并值或实例编号冲突。
    for content in [
        String::from_utf8(bytes.clone())
            .unwrap()
            .replace("snowflake_worker_id=2", "snowflake_worker_id=3"),
        String::from_utf8(bytes)
            .unwrap()
            .replace("health_port=18081", "health_port=18082"),
    ] {
        fs::write(&worker, content).unwrap();
        let api = runtime::configuration_digest(&alternate.join("config")).unwrap();
        let worker = runtime::configuration_digest(&alternate.join("worker-config")).unwrap();
        receipt["api_configuration_sha256"] = json!(api);
        receipt["worker_configuration_sha256"] = json!(worker);
        receipt["configuration_sha256"] =
            json!(python_configuration_hash(&json!([api, worker])).unwrap());
        fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        assert!(runtime::verify_configuration(root.path(), &directory).is_err());
    }
}

#[test]
fn configuration_only_contract_rejects_invalid_shared_scope_and_worker_settings() {
    let root = source_fixture_root();
    let (directory, receipt) = runtime_fixture(root.path());
    runtime::verify_configuration(root.path(), &directory).unwrap();
    let configs = ["config", "worker-config"].map(|name| root.path().join(name));
    let originals = configs
        .each_ref()
        .map(|path| fs::read_to_string(path.join("app.test.toml")).unwrap());
    let endpoint_error = "全栈 Worker 就绪端点与绑定配置不匹配";
    let port_error = "jobs.health_port 无效";
    for (setting, replacement, scope, reason) in [
        ("mode='external'", "mode='embedded'", None, endpoint_error),
        (
            "health_host='127.0.0.1'",
            "health_host='192.0.2.1'",
            None,
            endpoint_error,
        ),
        ("health_port=18081", "health_port=0", None, port_error),
        ("health_port=18081", "health_port=65536", None, port_error),
        (
            "scope_id='artifact-test'",
            "scope_id='../scope'",
            Some("../scope"),
            "全栈运行 scope 与配置不匹配",
        ),
    ] {
        for (path, original) in configs.iter().zip(&originals) {
            let changed = original.replace(setting, replacement);
            assert_ne!(changed.as_str(), original.as_str(), "{setting}");
            fs::write(path.join("app.test.toml"), changed).unwrap();
        }
        let [api, worker] = configs
            .each_ref()
            .map(|path| runtime::configuration_digest(path).unwrap());
        let mut receipt = receipt.clone();
        receipt["configuration_sha256"] =
            json!(python_configuration_hash(&json!([api, worker])).unwrap());
        receipt["api_configuration_sha256"] = json!(api);
        receipt["worker_configuration_sha256"] = json!(worker);
        if let Some(scope) = scope {
            receipt["scope_id"] = json!(scope);
        }
        fs::write(
            directory.join("runtime.json"),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        let error = runtime::verify_configuration(root.path(), &directory)
            .err()
            .unwrap();
        assert!(error.to_string().contains(reason), "{replacement}: {error}");
    }
}

fn source_fixture_root() -> tempfile::TempDir {
    let parent = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&parent).unwrap();
    tempfile::Builder::new()
        .prefix("artifact-source-")
        .tempdir_in(parent)
        .unwrap()
}

fn source_fixture(root: &Path) -> std::path::PathBuf {
    let (directory, _) = runtime_fixture(root);
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    fs::write(
        root.join(".gitignore"),
        "runtime/\nconfig/\nworker-config/\n/.local-tests/\n",
    )
    .unwrap();
    fs::write(root.join("source.txt"), b"source").unwrap();
    git(&["init", "--quiet"]);
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.test",
        "commit",
        "--quiet",
        "-m",
        "fixture",
    ]);
    let snapshot = crate::recovery::clone::native::source::capture(root)
        .unwrap()
        .source
        .snapshot;
    let source = json!({"head":snapshot.head,"patch_sha256":snapshot.patch_sha256,"files":[]});
    let pair_path = directory.join("source-pair.json");
    let ci_number = |name| {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(1)
    };
    let mut pair = json!({"format_version":1,"backend_sha":source["head"],"frontend_sha":source["head"],
        "run_id":ci_number("GITHUB_RUN_ID"),"attempt":ci_number("GITHUB_RUN_ATTEMPT"),
        "sources":{"backend":source,"frontend":source}});
    fs::write(&pair_path, serde_json::to_vec(&pair).unwrap()).unwrap();
    pair["receipt"] = super::super::file_binding(&pair_path, "pair").unwrap();
    let source = json!({"format_version":1,"fixture":"core","backend_root":root,"source_pair":pair,"original":{"backend":source}});
    let binaries: Value =
        serde_json::from_slice(&fs::read(directory.join("binaries.json")).unwrap()).unwrap();
    let artifacts = binaries
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, path)| {
            (
                name.clone(),
                super::super::file_binding(Path::new(path.as_str().unwrap()), "binary").unwrap(),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let build_path = directory.join("build-evidence.json");
    let build = json!({"format_version":1,"kind":"full-stack-build","backend_root":root,"source":source,"artifacts":artifacts,"build_jobs":4});
    fs::write(&build_path, serde_json::to_vec(&build).unwrap()).unwrap();
    let runtime_evidence = json!({"format_version":1,"kind":"full-stack-runtime","source":source,"build_evidence":super::super::file_binding(&build_path,"build").unwrap(),"runtime":super::super::file_binding(&directory.join("runtime.json"),"runtime").unwrap()});
    fs::write(
        directory.join("runtime-evidence.json"),
        serde_json::to_vec(&runtime_evidence).unwrap(),
    )
    .unwrap();
    directory
}

fn registration_fixture(root: &Path) -> std::path::PathBuf {
    let directory = source_fixture(root);
    for name in ["runtime.json", "runtime-evidence.json"] {
        fs::remove_file(directory.join(name)).unwrap();
    }
    directory
}

fn register_fixture(root: &Path, directory: &Path) -> Result<Value> {
    runtime::register(
        root,
        directory,
        &root.join("config"),
        &root.join("worker-config"),
    )
}

fn assert_registration_absent(directory: &Path) {
    for name in ["runtime.json", "runtime-evidence.json"] {
        assert_eq!(
            fs::symlink_metadata(directory.join(name))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound,
            "{name}"
        );
    }
}

#[test]
fn runtime_register_checks_all_frozen_artifacts_and_current_jobs_before_writing() {
    let root = source_fixture_root();
    let directory = registration_fixture(root.path());
    for role in [
        "ryframe",
        "ryframe-worker",
        "ryframe-reset",
        "ryframe-migrate",
    ] {
        let binary = directory.join(format!("{role}.exe"));
        let original = fs::read(&binary).unwrap();
        fs::write(&binary, vec![b'x'; original.len()]).unwrap();
        assert_registration_absent(&directory);
        let error = register_fixture(root.path(), &directory).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("构建证据与二进制清单或产物不匹配"),
            "{role}: {error}"
        );
        assert_registration_absent(&directory);
        fs::write(binary, original).unwrap();
    }
    fs::create_dir(root.path().join(".local-tests")).unwrap();
    fs::write(
        root.path().join(".local-tests/tools.toml"),
        "[verification]\nbuild_jobs=1\n",
    )
    .unwrap();
    let error = register_fixture(root.path(), &directory).unwrap_err();
    assert!(
        error.to_string().contains("verification.build_jobs"),
        "{error}"
    );
    assert_registration_absent(&directory);
}

#[test]
fn runtime_register_rejects_manifest_extras_and_source_errors_before_writing() {
    let root = source_fixture_root();
    let directory = registration_fixture(root.path());
    let manifest = directory.join("binaries.json");
    let bytes = fs::read(&manifest).unwrap();
    let mut binaries: Value = serde_json::from_slice(&bytes).unwrap();
    binaries["unexpected"] = binaries["ryframe"].clone();
    fs::write(&manifest, serde_json::to_vec(&binaries).unwrap()).unwrap();
    let error = register_fixture(root.path(), &directory).unwrap_err();
    assert!(error.to_string().contains("全栈二进制清单"), "{error}");
    assert_registration_absent(&directory);
    fs::write(manifest, bytes).unwrap();

    let build_path = directory.join("build-evidence.json");
    let bytes = fs::read(&build_path).unwrap();
    let mut build: Value = serde_json::from_slice(&bytes).unwrap();
    build["source"]["source_pair"]["run_id"] = json!("other-run");
    fs::write(&build_path, serde_json::to_vec(&build).unwrap()).unwrap();
    let error = register_fixture(root.path(), &directory).unwrap_err();
    assert!(
        error.to_string().contains("构建来源与源码组合不匹配"),
        "{error}"
    );
    assert_registration_absent(&directory);
    fs::write(build_path, bytes).unwrap();

    fs::remove_file(directory.join("source-pair.json")).unwrap();
    let error = register_fixture(root.path(), &directory).unwrap_err();
    assert!(error.to_string().contains("运行登记来源"), "{error}");
    assert_registration_absent(&directory);
}

#[test]
fn runtime_register_publishes_verified_core_source_only_once() {
    let root = source_fixture_root();
    let directory = registration_fixture(root.path());
    assert_registration_absent(&directory);
    let password = "artifact-runtime-fixture-password";
    for role in ["config", "worker-config"] {
        let path = root.path().join(role).join("app.test.toml");
        let mut content = fs::read_to_string(&path).unwrap();
        content.push_str(&format!("\n[database.primary]\npassword='{password}'\n"));
        fs::write(path, content).unwrap();
    }
    let receipt = register_fixture(root.path(), &directory).unwrap();
    let api_configuration = root.path().join("config");
    let digest = runtime::configuration_digest(&api_configuration).unwrap();
    fs::write(api_configuration.join("notes.txt"), "辅助文本").unwrap();
    assert_eq!(
        runtime::configuration_digest(&api_configuration).unwrap(),
        digest
    );
    let verified = runtime::verify(root.path(), &directory).unwrap();
    assert_eq!(verified.receipt, receipt);
    assert_eq!(
        verified.table["database"]["primary"]["password"].as_str(),
        Some(password)
    );
    let outputs = ["runtime.json", "runtime-evidence.json"].map(|name| directory.join(name));
    let before = outputs.each_ref().map(|path| fs::read(path).unwrap());
    for bytes in &before {
        assert!(!std::str::from_utf8(bytes).unwrap().contains(password));
    }
    assert!(register_fixture(root.path(), &directory).is_err());
    assert_eq!(
        outputs.each_ref().map(|path| fs::read(path).unwrap()),
        before
    );
}

#[test]
fn artifact_source_rejects_source_and_receipt_drift() {
    let root = source_fixture_root();
    let directory = source_fixture(root.path());
    let receipt = runtime::verify(root.path(), &directory).unwrap().receipt;
    let runtime_path = directory.join("runtime.json");
    let evidence_path = directory.join("runtime-evidence.json");
    let runtime_bytes = fs::read(&runtime_path).unwrap();
    let evidence_bytes = fs::read(&evidence_path).unwrap();
    fs::remove_file(&runtime_path).unwrap();
    let error = runtime::verify(root.path(), &directory).err().unwrap();
    assert!(
        error.to_string().contains("无法读取全栈运行收据"),
        "{error}"
    );
    assert_eq!(
        fs::symlink_metadata(&runtime_path).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    assert_eq!(fs::read(&evidence_path).unwrap(), evidence_bytes);

    let wrong_root =
        crate::recovery::fixture_control::portable_path(&directory.canonicalize().unwrap());
    let identity_error = "全栈运行收据版本或后端目录不匹配";
    for (key, value, reason) in [
        ("format_version", json!(2), identity_error),
        ("extra", json!(true), "全栈运行收据字段缺失或包含未知字段"),
        ("backend_root", json!(wrong_root), identity_error),
    ] {
        let mut changed = receipt.clone();
        changed[key] = value;
        let bytes = serde_json::to_vec(&changed).unwrap();
        fs::write(&runtime_path, &bytes).unwrap();
        let error = runtime::verify(root.path(), &directory).err().unwrap();
        assert!(error.to_string().contains(reason), "{key}: {error}");
        assert_eq!(fs::read(&runtime_path).unwrap(), bytes);
        assert_eq!(fs::read(&evidence_path).unwrap(), evidence_bytes);
    }
    fs::write(runtime_path, runtime_bytes).unwrap();
    fs::write(root.path().join("source.txt"), b"drift").unwrap();
    assert!(runtime::verify(root.path(), &directory).is_err());
    assert!(runtime::verify_configuration(root.path(), &directory).is_ok());
    fs::write(root.path().join("source.txt"), b"source").unwrap();
    fs::write(directory.join("runtime-evidence.json"), b"{}").unwrap();
    assert!(runtime::verify(root.path(), &directory).is_err());
}

#[test]
fn artifact_source_rejects_unconsumed_snapshot_fields_with_consistent_bindings() {
    let root = source_fixture_root();
    let directory = source_fixture(root.path());
    super::source::verify(root.path(), &directory).unwrap();
    let pair_path = directory.join("source-pair.json");
    let mut pair = super::super::read_json(&pair_path, "pair", 16 * 1024).unwrap();
    pair["sources"]["frontend"]["unconsumed"] = json!("must reject");
    fs::write(&pair_path, serde_json::to_vec(&pair).unwrap()).unwrap();
    pair["receipt"] = super::super::file_binding(&pair_path, "pair").unwrap();
    let build_path = directory.join("build-evidence.json");
    let mut build = super::super::read_json(&build_path, "build", 4 * 1024 * 1024).unwrap();
    build["source"]["source_pair"] = pair;
    fs::write(&build_path, serde_json::to_vec(&build).unwrap()).unwrap();
    let evidence_path = directory.join("runtime-evidence.json");
    let mut evidence = super::super::read_json(&evidence_path, "runtime", 4 * 1024 * 1024).unwrap();
    evidence["source"] = build["source"].clone();
    evidence["build_evidence"] = super::super::file_binding(&build_path, "build").unwrap();
    fs::write(&evidence_path, serde_json::to_vec(&evidence).unwrap()).unwrap();
    let error = super::source::verify(root.path(), &directory)
        .unwrap_err()
        .to_string();
    assert!(error.contains("原始来源快照"), "{error}");
    assert!(error.contains("字段"), "{error}");
}

#[test]
fn artifact_source_rechecks_source_and_all_consumed_receipts_after_observation() {
    let root = source_fixture_root();
    let directory = source_fixture(root.path());
    let observed = std::cell::Cell::new(0);
    super::source::verify_observed(root.path(), &directory, || observed.set(observed.get() + 1))
        .unwrap();
    assert_eq!(observed.get(), 1);
    for name in [
        "build-evidence.json",
        "source-pair.json",
        "runtime.json",
        "runtime-evidence.json",
        "binaries.json",
    ] {
        let path = directory.join(name);
        let original = fs::read(&path).unwrap();
        observed.set(0);
        let error = super::source::verify_observed(root.path(), &directory, || {
            observed.set(observed.get() + 1);
            let mut changed = original.clone();
            changed.push(b'\n');
            fs::write(&path, changed).unwrap();
        })
        .unwrap_err()
        .to_string();
        assert_eq!(observed.get(), 1, "{name}");
        assert!(
            error.contains("发生变化") || error.contains("构建来源与源码组合不匹配"),
            "{name}: {error}"
        );
        assert_ne!(fs::read(&path).unwrap(), original, "{name}");
        fs::write(path, original).unwrap();
    }
    let path = root.path().join("source.txt");
    let error = super::source::verify_observed(root.path(), &directory, || {
        fs::write(&path, b"changed after artifacts").unwrap();
    })
    .unwrap_err()
    .to_string();
    assert!(error.contains("core 来源与当前源码不匹配"), "{error}");
    assert_eq!(fs::read(&path).unwrap(), b"changed after artifacts");
    fs::write(path, b"source").unwrap();
    let evidence = directory.join("runtime-evidence.json");
    fs::write(evidence, b"{}").unwrap();
    observed.set(0);
    assert!(super::source::verify_observed(root.path(), &directory, || observed.set(1)).is_err());
    assert_eq!(observed.get(), 0);
}
