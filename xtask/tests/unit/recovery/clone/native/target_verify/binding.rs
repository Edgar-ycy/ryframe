//! 退休传输后保留配置文件与收尾证据的直接校验，不执行资源命令。
use super::{private_environment, reconciliation};
use crate::recovery::clone::native::resources::common;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn workspace() -> tempfile::TempDir {
    let parent = crate::workspace::root_dir().join(".local-tests/target-binding-unit");
    fs::create_dir_all(&parent).unwrap();
    let directory = tempfile::tempdir_in(parent).unwrap();
    fs::create_dir(directory.path().join(".local-tests")).unwrap();
    directory
}

fn write(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn digest(path: &Path) -> Value {
    let mut value = common::binding(path).unwrap();
    value.as_object_mut().unwrap().remove("path");
    value
}

#[test]
fn private_environment_is_file_bound_and_rejects_retired_configuration_and_case_duplicates() {
    let directory = workspace();
    let backend = directory.path();
    let temporary = backend.join(".local-tests/temp");
    fs::create_dir(&temporary).unwrap();
    let file = backend.join(".local-tests/private.json");
    let valid = json!({"APP_ENV":"test","TEMP":temporary,"TMP":temporary});
    write(&file, &json!({"environment":valid}));
    let original = common::binding(&file).unwrap();
    assert_eq!(
        private_environment(backend, &original).unwrap()["environment"],
        valid
    );
    for (key, value) in [
        ("APP_SCOPE_ID", json!("secret-never-echo")),
        ("RYFRAME_RESET_ADMIN_PASSWORD", json!("secret-never-echo")),
        ("PATH", json!("forbidden")),
        ("app_env", json!("dev")),
        ("TEMP", json!(1)),
    ] {
        let mut invalid = valid.clone();
        invalid[key] = value;
        write(&file, &json!({"environment":invalid}));
        let error = private_environment(backend, &common::binding(&file).unwrap()).unwrap_err();
        assert!(!error.to_string().contains("secret-never-echo"));
    }
    assert!(private_environment(backend, &original).is_err());
}

#[test]
fn private_environment_rejects_outside_missing_mismatched_and_file_temp() {
    let directory = workspace();
    let backend = directory.path();
    let file = backend.join(".local-tests/private.json");
    let temporary = backend.join(".local-tests/temp");
    fs::create_dir(&temporary).unwrap();
    for (temp, tmp) in [
        (temporary, backend.join(".local-tests/missing")),
        (
            backend.parent().unwrap().to_path_buf(),
            backend.parent().unwrap().to_path_buf(),
        ),
        (file.clone(), file.clone()),
        (PathBuf::from("relative"), PathBuf::from("relative")),
    ] {
        write(&file, &json!({"environment":{"TEMP":temp,"TMP":tmp}}));
        assert!(private_environment(backend, &common::binding(&file).unwrap()).is_err());
    }
}

#[test]
fn preflight_receipt_requires_exact_bound_failure_report_and_resource_before_after() {
    let directory = workspace();
    let backend = directory.path();
    let target = backend.join(".local-tests/target");
    fs::create_dir_all(target.join("reset-state")).unwrap();
    write(
        &target.join("failure.json"),
        &json!({"status":"needs_reconciliation"}),
    );
    let report = target.join("reset-state/one.report.json");
    write(
        &report,
        &json!({"status":"failed","failed_phase":"preflight"}),
    );
    let completed = target.join("reconciliation-completed.json");
    let value = json!({"status":"preflight_failure_reconciled","before":{
        "databases":[{"exists":true}],"objects":{},"redis":{}},"after":{
        "databases":[{"exists":false}],"objects":{},"redis":{}},
        "failure":digest(&target.join("failure.json")),"reset_report":digest(&report),
        "automatic_retry":false,"restore_qualified":false});
    write(&completed, &value);
    let before = common::binding(&completed).unwrap();
    let mut expected = before.clone();
    expected["path"] = json!(completed);
    assert_eq!(
        reconciliation(backend, &target).unwrap()["binding"],
        expected
    );
    assert_eq!(common::binding(&completed).unwrap(), before);
    for pointer in ["/automatic_retry", "/after/databases/0/exists"] {
        let mut invalid = value.clone();
        *invalid.pointer_mut(pointer).unwrap() = json!(true);
        write(&completed, &invalid);
        assert!(reconciliation(backend, &target).is_err());
    }
    write(&completed, &value);
    write(&report, &json!({"changed":"secret-never-echo"}));
    assert!(reconciliation(backend, &target).is_err());
    write(&target.join("reset-state/two.report.json"), &json!({}));
    assert!(reconciliation(backend, &target).is_err());
}
