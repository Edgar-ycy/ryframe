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
fn bind_resolves_equivalent_input_spelling_and_preserves_launch_paths() {
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
    for file in [&bindings, &backend_build, &frontend_build] {
        fs::write(file, b"{}").unwrap();
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
    let value = json!({
        "runtime_directory":root,
        "request":{
            "roots":{"backend_product":product,"backend_execution":execution,"frontend":frontend},
            "paths":{"bindings":bindings,"backend_build":backend_build,"frontend_build":frontend_build},
        },
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
    assert_eq!(
        receipt["paths"]["backend_product_root"],
        request["roots"]["backend_product"]
    );
    assert_eq!(
        receipt["paths"]["backend_execution_root"],
        request["roots"]["backend_execution"]
    );
    assert_eq!(
        receipt["paths"]["frontend_root"],
        request["roots"]["frontend"]
    );
    for field in ["bindings", "backend_build", "frontend_build"] {
        assert_eq!(receipt["paths"][field], request["paths"][field]);
    }

    options.bindings = root.join("other-bindings.json");
    fs::copy(&bindings, &options.bindings).unwrap();
    assert!(BoundPaths::read(&options, request).is_err());
    options.bindings = root.join("hard-link-bindings.json");
    fs::hard_link(&bindings, &options.bindings).unwrap();
    assert!(BoundPaths::read(&options, request).is_err());
}
