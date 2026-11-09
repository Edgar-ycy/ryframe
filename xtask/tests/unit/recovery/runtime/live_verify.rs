use std::fs;

use super::*;

fn workspace() -> tempfile::TempDir {
    let root = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

#[test]
fn live_verify_rejects_a_different_receipt_digest_before_claiming_owner() -> Result<()> {
    use crate::{
        cli::RuntimeSources,
        recovery::{fixture_control::portable_path, runtime::REGISTRATION_GUARD},
    };

    let workspace = workspace();
    let root = portable_path(&workspace.path().canonicalize()?);
    let local = root.join(".local-tests");
    fs::create_dir(&local)?;
    let receipt = local.join("runtime.json");
    let bindings = local.join("bindings.json");
    for path in [&receipt, &bindings] {
        fs::write(path, b"{}")?;
    }
    let options = RuntimeVerifyOptions {
        sources: RuntimeSources {
            source_backend: root.clone(),
            source_frontend: root.clone(),
            product_backend: None,
            adapter_contract: None,
        },
        bindings,
        receipt,
    };
    let window = Window::begin()?;
    let error = verify_live_generation(&root, &options, &Value::Null, &"0".repeat(64), &window)
        .unwrap_err();
    assert!(error.to_string().contains("实际 SHA-256 与给定摘要不同"));
    assert!(!local.join(REGISTRATION_GUARD).exists());
    window.finish(Ok(()))
}

#[test]
fn verify_window_checks_only_its_own_control_additions() -> Result<()> {
    let workspace = workspace();
    fs::create_dir(workspace.path().join(".local-tests"))?;
    let control = control_root(workspace.path())?;
    let registration = "a".repeat(64);
    let target = "b".repeat(64);
    let operation = Operation::Verify.describe(&registration, &target, 2);
    assert_eq!(
        operation,
        format!("runtime-verify:{registration}:{target}:2")
    );
    let window = Window::begin()?;
    reads::directory(&control)?;
    Owner::with(&control, &operation, |owner| {
        assert!(window.finish(Ok(())).is_err());
        window.finish_runtime_owner(owner, Ok(()))?;
        let unexpected = control.join("foreign.json");
        fs::write(&unexpected, b"{}")?;
        assert!(window.finish_runtime_owner(owner, Ok(())).is_err());
        fs::remove_file(unexpected)?;
        window.finish_runtime_owner(owner, Ok(()))
    })?;
    // owner 已释放，调用方可继续复用同一读窗。
    window.finish(Ok(()))
}

#[test]
fn verify_window_accepts_exact_control_members_already_observed_under_lock() -> Result<()> {
    let workspace = workspace();
    fs::create_dir(workspace.path().join(".local-tests"))?;
    let control = control_root(workspace.path())?;
    let operation = Operation::Verify.describe(&"a".repeat(64), &"b".repeat(64), 1);
    Owner::with(&control, &operation, |owner| {
        let window = Window::begin()?;
        reads::directory(&control)?;
        window.finish_runtime_owner(owner, Ok(()))
    })
}
