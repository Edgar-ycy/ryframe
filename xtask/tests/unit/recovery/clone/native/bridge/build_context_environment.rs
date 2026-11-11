use super::*;
use serde_json::json;

#[test]
fn registered_environment_retains_recorded_names_and_rejects_invalid_values() {
    let path = if cfg!(windows) { "Path" } else { "PATH" };
    let values = environment_values(&json!({
        (path): "recorded search path",
        "CARGO_ENCODED_RUSTFLAGS": "-C\u{1f}opt-level=2",
    }))
    .unwrap();
    assert_eq!(entry(&values, "PATH"), Some("recorded search path"));
    assert!(values.contains_key(path));
    for invalid in [
        json!({"APP_SECRET": "private"}),
        json!({"CC_=bad": "private"}),
        json!({"RUSTC": true}),
        json!({"PATH": "\0private"}),
    ] {
        let error = environment_values(&invalid).unwrap_err();
        assert!(!error.to_string().contains("private"));
    }
}

#[cfg(windows)]
#[test]
fn case_aliases_are_rejected_before_windows_tool_lookup() {
    assert!(environment_values(&json!({"PATH": "first", "Path": "second"})).is_err());
    assert!(environment_values(&json!({"RUSTC": "first", "rustc": "second"})).is_err());
}

#[test]
fn first_path_executable_cannot_be_replaced_by_later_matching_bytes() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
    let first_cargo = first.join(name);
    let second_cargo = second.join(name);
    for path in [&first_cargo, &second_cargo] {
        fs::write(path, b"same executable bytes").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let path = env::join_paths([root.path().join("absent"), first, second]).unwrap();
    let selected = first_executable(root.path(), path.to_str().unwrap(), "cargo").unwrap();
    assert_eq!(selected, first_cargo);
    assert!(same_executable(&selected, &first_cargo).is_ok());
    assert!(same_executable(&selected, &second_cargo).is_err());
}

#[test]
fn malformed_private_document_fails_without_echoing_duplicate_variable_name() {
    let root = tempfile::tempdir().unwrap();
    let local = root.path().join(".local-tests");
    fs::create_dir(&local).unwrap();
    let path = local.join("build-environment.json");
    fs::write(
        &path,
        br#"{"private-name":"private-value","private-name":1}"#,
    )
    .unwrap();
    let error = verify_registered(root.path(), &path, "fixture", &Value::Null, &Value::Null)
        .unwrap_err()
        .to_string();
    assert!(error.contains("严格 UTF-8 JSON"));
    assert!(!error.contains("private"));
}
