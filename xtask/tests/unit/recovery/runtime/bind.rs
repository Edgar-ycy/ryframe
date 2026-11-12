use std::fs;

use super::*;
use crate::recovery::{
    clone::native::{json as strict, lifecycle::lock::ProcessGuard},
    runtime::{REGISTRATION_GUARD, REGISTRATION_LOCK},
};

const OPERATION: &str = "runtime-bind:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb:2";

fn workspace() -> tempfile::TempDir {
    let root = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

#[test]
fn bind_owner_uses_the_existing_guard_and_recover_receipt() {
    let work = workspace();
    let root = portable_path(&work.path().canonicalize().unwrap());
    let path = root.join(REGISTRATION_LOCK).join("owner.json");
    Owner::with(&root, OPERATION, |owner| {
        owner.check()?;
        let receipt = strict::read(&path)?;
        strict::exact(
            &receipt,
            &[
                "format_version",
                "kind",
                "identity",
                "runtime_directory",
                "operation",
                "token",
            ],
            "恢复运行 owner",
        )?;
        assert_eq!(receipt["format_version"], 1);
        assert_eq!(receipt["kind"], "restore-runtime-registration-controller");
        assert_eq!(receipt["operation"], OPERATION);
        assert_eq!(receipt["runtime_directory"], json!(root));
        assert_eq!(receipt["identity"]["pid"], std::process::id());
        let token = receipt["token"].as_str().unwrap();
        assert!(
            token.len() == 32
                && token
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        let contender = ProcessGuard::acquire_named(&root, REGISTRATION_GUARD);
        assert!(contender.err().unwrap().to_string().contains("并发"));
        assert_eq!(strict::read(&path)?, receipt);
        Ok(())
    })
    .unwrap();
    assert!(!path.exists());
    assert!(!root.join(REGISTRATION_LOCK).exists());
    assert_eq!(fs::read(root.join(REGISTRATION_GUARD)).unwrap(), [0]);
    ProcessGuard::acquire_named(&root, REGISTRATION_GUARD).unwrap();
    let error = Owner::with(&root, OPERATION, |_| -> Result<()> {
        Err("受管失败".into())
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "受管失败");
    assert!(!root.join(REGISTRATION_LOCK).exists());
}

#[test]
fn bind_refuses_stale_owner_and_preserves_unknown_change_after_error() {
    let work = workspace();
    let root = portable_path(&work.path().canonicalize().unwrap());
    let directory = root.join(REGISTRATION_LOCK);
    let path = directory.join("owner.json");
    fs::create_dir(&directory).unwrap();
    fs::write(&path, b"{\"stale\":true}").unwrap();
    assert!(
        Owner::with(&root, OPERATION, |_| -> Result<()> {
            panic!("不能接管未核对的旧 owner")
        })
        .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), b"{\"stale\":true}");
    fs::remove_file(&path).unwrap();
    fs::remove_dir(&directory).unwrap();
    let error = Owner::with(&root, OPERATION, |_| -> Result<()> {
        fs::write(&path, b"{\"foreign\":true}")?;
        Err("保留原失败".into())
    })
    .unwrap_err();
    assert!(error.to_string().contains("保留原失败"));
    assert!(error.to_string().contains("ownership"));
    assert_eq!(fs::read(&path).unwrap(), b"{\"foreign\":true}");
    ProcessGuard::acquire_named(&root, REGISTRATION_GUARD).unwrap();
}

#[test]
fn named_guard_rejects_hardlinks_and_bind_output_stays_outside_generation() {
    let work = workspace();
    let root = portable_path(&work.path().canonicalize().unwrap());
    let source = root.join("marker");
    fs::write(&source, [0]).unwrap();
    fs::hard_link(&source, root.join(REGISTRATION_GUARD)).unwrap();
    assert!(ProcessGuard::acquire_named(&root, REGISTRATION_GUARD).is_err());
    assert_eq!(fs::read(&source).unwrap(), [0]);

    let runtime = root.join("runtime");
    fs::create_dir(&runtime).unwrap();
    assert!(validate_output_boundary(&runtime, &runtime).is_err());
    assert!(
        validate_output_boundary(&runtime.join("generation-0001/runtime.json"), &runtime).is_err()
    );
    validate_output_boundary(&root.join("runtime.json"), &runtime).unwrap();
}

#[test]
fn bind_resolves_equivalent_input_spelling_and_preserves_runtime_receipt() {
    use crate::recovery::inputs::document::Document;

    let work = workspace();
    let root = portable_path(&work.path().canonicalize().unwrap());
    let execution = root.join("execution");
    let product = root.join("product");
    let frontend = root.join("frontend");
    fs::create_dir(&execution).unwrap();
    fs::create_dir(&product).unwrap();
    fs::create_dir_all(frontend.join("dist/.vite")).unwrap();
    let bindings = root.join("bindings.json");
    let backend_build = root.join("backend-build.json");
    let frontend_build = frontend.join("dist/.vite/restore-build.json");
    for (file, raw) in [
        (&bindings, br#"{"record":"binding"}"#.as_slice()),
        (&backend_build, br#"{"build":"backend"}"#.as_slice()),
        (&frontend_build, br#"{"build":"frontend"}"#.as_slice()),
    ] {
        fs::write(file, raw).unwrap();
    }
    let spelling = |path: &Path| PathBuf::from(path.to_string_lossy().replace('\\', "/"));
    #[cfg(windows)]
    assert_ne!(spelling(&execution).as_os_str(), execution.as_os_str());
    let mut options = RuntimeBindOptions {
        sources: RuntimeSources {
            source_backend: spelling(&execution),
            source_frontend: spelling(&frontend),
            product_backend: Some(spelling(&product)),
            adapter_contract: Some("legacy-stable-readiness-b0-v1".to_owned()),
        },
        build_receipt: spelling(&backend_build),
        launch_receipt: root.join("runtime-launch.json"),
        bindings: spelling(&bindings),
        output: root.join("runtime.json"),
    };
    let process_receipts = bind_process_receipts(&root);
    let value = json!({
        "runtime_directory":root,
        "request":{
            "roots":{"backend_product":product,"backend_execution":execution,"frontend":frontend},
            "paths":{"bindings":bindings,"backend_build":backend_build,"frontend_build":frontend_build},
            "authority":{
                "restore_id":"restore-bind","backup_id":"backup-bind","scope_id":"bind-fixture",
                "plan_hash":"1".repeat(64),"data_verified_at":"2026-11-12T03:10:09Z",
                "backup_source_sha":"2".repeat(40),"backend_product_sha":"3".repeat(40),
                "backend_execution_sha":"4".repeat(40),"frontend_sha":"5".repeat(40),
                "backend_adapter_contract":"legacy-stable-readiness-b0-v1",
                "api_endpoint":"http://127.0.0.1:18180/readyz",
                "worker_endpoint":"http://127.0.0.1:18181/readyz",
                "frontend_endpoint":"http://127.0.0.1:15174",
            },
        },
        "processes":process_receipts,
    });
    let control = Control {
        launch: Document {
            path: options.launch_receipt.clone(),
            raw: serde_json::to_vec(&value).unwrap(),
            value,
        },
        directory: root.clone(),
        operation: OPERATION.to_owned(),
        snapshot: Value::Null,
    };
    let request = &control.launch.value["request"];
    let bound = BoundPaths::read(&options, request).unwrap();
    let receipt = runtime_receipt(&bound, &control, &mut Inputs::new(&root)).unwrap();
    assert_bind_receipt(&receipt, &bound, &control);

    options.bindings = root.join("other-bindings.json");
    fs::copy(&bindings, &options.bindings).unwrap();
    assert!(BoundPaths::read(&options, request).is_err());
    options.bindings = root.join("hard-link-bindings.json");
    fs::hard_link(&bindings, &options.bindings).unwrap();
    assert!(BoundPaths::read(&options, request).is_err());
}

fn bind_process_receipts(root: &Path) -> Value {
    let mut processes = json!({});
    for (ordinal, role) in generation::ROLES.into_iter().enumerate() {
        processes[role] = json!({"process_receipt":{
            "path":root.join(format!("{role}.json")),
            "sha256":(ordinal + 6).to_string().repeat(64),
            "identity":{"pid":100 + ordinal,"started":(10 + ordinal).to_string(),
                "executable":root.join(format!("{role}-fixture"))},
        }});
    }
    processes
}

fn assert_bind_receipt(receipt: &Value, bound: &BoundPaths, control: &Control) {
    use crate::recovery::clone::native::digest;
    use crate::recovery::runtime::dataset_authority::schema;

    schema::runtime(receipt).unwrap();
    assert_eq!(receipt["format_version"], 3);
    assert_eq!(receipt["kind"], "restore-runtime");
    assert_eq!(
        receipt["restore"],
        json!({"id":"restore-bind","backup_id":"backup-bind","scope_id":"bind-fixture",
            "plan_hash":"1".repeat(64),"data_verified_at":"2026-11-12T03:10:09Z"})
    );
    assert_eq!(
        receipt["source"],
        json!({"backup_source_sha":"2".repeat(40),"backend_product_sha":"3".repeat(40),
            "backend_execution_sha":"4".repeat(40),"frontend_sha":"5".repeat(40),
            "backend_adapter_contract":"legacy-stable-readiness-b0-v1"})
    );
    assert_eq!(
        receipt["endpoints"],
        json!({"api":"http://127.0.0.1:18180/readyz","worker":"http://127.0.0.1:18181/readyz",
            "frontend":"http://127.0.0.1:15174"})
    );
    let request = &control.launch.value["request"];
    assert_eq!(
        receipt["paths"],
        json!({"backend_product_root":request["roots"]["backend_product"],
            "backend_execution_root":request["roots"]["backend_execution"],
            "frontend_root":request["roots"]["frontend"],"runtime_dir":control.directory,
            "bindings":request["paths"]["bindings"],"backend_build":request["paths"]["backend_build"],
            "frontend_build":request["paths"]["frontend_build"],
            "launch":control.launch.path})
    );
    for (key, path) in [
        ("bindings", &bound.bindings),
        ("backend_build", &bound.backend),
        ("frontend_build", &bound.frontend),
    ] {
        assert_eq!(receipt["digests"][key], digest::file(path).unwrap().sha256);
    }
    assert_eq!(receipt["digests"]["launch"], control.launch.sha256());
    assert_eq!(receipt["backend"], json!({"build":"backend"}));
    assert_eq!(receipt["frontend"], json!({"build":"frontend"}));
    for role in generation::ROLES {
        let record = &control.launch.value["processes"][role]["process_receipt"];
        assert_eq!(
            receipt["processes"][role],
            json!({"receipt_path":record["path"],"receipt_sha256":record["sha256"],
                "identity":record["identity"]})
        );
    }
}

#[test]
fn bind_publication_keeps_the_same_owner_and_preserves_the_receipt_after_a_failed_recheck() {
    use crate::recovery::inputs::document;

    let work = workspace();
    let root = portable_path(&work.path().canonicalize().unwrap());
    let owner_path = root.join(REGISTRATION_LOCK).join("owner.json");
    let output = root.join("runtime.json");
    let runtime = json!({"format_version":3,"kind":"restore-runtime","fixture":"first"});
    let bytes = document::canonical_bytes(&runtime).unwrap();
    let mut rechecks = 0;

    let error = Owner::with(&root, OPERATION, |owner| {
        let owner_bytes = fs::read(&owner_path)?;
        publish_locked(&output, &runtime, owner, || {
            rechecks += 1;
            assert_eq!(fs::read(&owner_path)?, owner_bytes);
            let contender = ProcessGuard::acquire_named(&root, REGISTRATION_GUARD);
            assert!(contender.err().unwrap().to_string().contains("并发"));
            if rechecks == 1 {
                assert!(!output.try_exists()?);
                return Ok(());
            }
            assert_eq!(rechecks, 2);
            assert_eq!(fs::read(&output)?, bytes);
            Err("发布后核验失败".into())
        })
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "发布后核验失败");
    assert_eq!(rechecks, 2);
    assert!(!root.join(REGISTRATION_LOCK).exists());

    let mut replay_checks = 0;
    let replay = Owner::with(&root, OPERATION, |owner| {
        publish_locked(&output, &json!({"fixture":"replacement"}), owner, || {
            replay_checks += 1;
            Ok(())
        })
    });
    assert!(replay.unwrap_err().to_string().contains("禁止覆盖"));
    assert_eq!(replay_checks, 1);
    assert_eq!(fs::read(&output).unwrap(), bytes);
    assert!(!root.join(REGISTRATION_LOCK).exists());
    assert!(!fs::read_dir(&root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".pending")
    }));
    ProcessGuard::acquire_named(&root, REGISTRATION_GUARD).unwrap();
}
