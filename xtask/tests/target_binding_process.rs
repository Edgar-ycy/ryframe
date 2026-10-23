//! fresh CLI 实际受管协议：无 Python、无项目配置环境覆盖、无写入与凭据回显。

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn invoke(args: &[&str], input: &[u8]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    command
        .args(args)
        .env("PATH", "")
        .env("APP_ENV", "prod")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().unwrap();
    let _ = child.stdin.take().unwrap().write_all(input);
    child.wait_with_output().unwrap()
}

fn call(backend: &Path, operation: &str, arguments: Value) -> (bool, Value) {
    let bytes = serde_json::to_vec(&json!({"format_version":1,"operation":operation,
        "backend":backend,"arguments":arguments}))
    .unwrap();
    let output = invoke(&["__target-binding"], &bytes);
    assert!(output.stderr.is_empty());
    (
        output.status.success(),
        serde_json::from_slice(&output.stdout).unwrap(),
    )
}

fn workspace() -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let parent = root.join(".local-tests/target-binding-process");
    fs::create_dir_all(&parent).unwrap();
    let directory = tempfile::tempdir_in(parent).unwrap();
    fs::create_dir(directory.path().join(".local-tests")).unwrap();
    directory
}

fn write(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn descriptor(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap();
    json!({"path":path,"bytes":bytes.len(),"sha256":hex::encode(Sha256::digest(&bytes))})
}

fn digest(path: &Path) -> Value {
    let mut value = descriptor(path);
    value.as_object_mut().unwrap().remove("path");
    value
}

#[test]
fn binding_protocol_rejects_oversize_duplicate_unknown_and_argument_inputs() {
    for input in [
        Vec::new(),
        vec![b'x'; 1024 * 1024 + 1],
        br#"{"format_version":1,"format_version":1}"#.to_vec(),
        br#"{"format_version":1,"operation":"credential-secret"}"#.to_vec(),
    ] {
        let output = invoke(&["__target-binding"], &input);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stderr.is_empty());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["remote_writes"], 0);
        assert_eq!(value["error"]["automatic_retry"], false);
        assert!(
            !String::from_utf8(output.stdout)
                .unwrap()
                .contains("credential-secret")
        );
    }
    assert_eq!(
        invoke(&["__target-binding", "forbidden"], b"{}")
            .status
            .code(),
        Some(1)
    );
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
    let original = descriptor(&file);
    let (success, value) = call(
        backend,
        "private-environment",
        json!({"descriptor":original}),
    );
    assert!(success, "{value}");
    assert_eq!(value["result"]["environment"], valid);
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
        let (success, response) = call(
            backend,
            "private-environment",
            json!({"descriptor":descriptor(&file)}),
        );
        assert!(!success, "{key}");
        assert!(!response.to_string().contains("secret-never-echo"));
    }
    let (success, _) = call(
        backend,
        "private-environment",
        json!({"descriptor":original}),
    );
    assert!(!success);
}

#[test]
fn private_environment_rejects_outside_missing_mismatched_and_file_temp() {
    let directory = workspace();
    let backend = directory.path();
    let file = backend.join(".local-tests/private.json");
    let temporary = backend.join(".local-tests/temp");
    fs::create_dir(&temporary).unwrap();
    for (temp, tmp) in [
        (temporary.clone(), backend.join(".local-tests/missing")),
        (
            backend.parent().unwrap().to_path_buf(),
            backend.parent().unwrap().to_path_buf(),
        ),
        (file.clone(), file.clone()),
        (PathBuf::from("relative"), PathBuf::from("relative")),
    ] {
        write(&file, &json!({"environment":{"TEMP":temp,"TMP":tmp}}));
        assert!(
            !call(
                backend,
                "private-environment",
                json!({"descriptor":descriptor(&file)})
            )
            .0
        );
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
    let before = descriptor(&completed);
    let (success, result) = call(
        backend,
        "preflight-reconciliation",
        json!({"target":target}),
    );
    assert!(success, "{result}");
    assert_eq!(result["result"]["binding"], before);
    assert_eq!(descriptor(&completed), before);
    for pointer in ["/automatic_retry", "/after/databases/0/exists"] {
        let mut invalid = value.clone();
        *invalid.pointer_mut(pointer).unwrap() = json!(true);
        write(&completed, &invalid);
        assert!(
            !call(
                backend,
                "preflight-reconciliation",
                json!({"target":target})
            )
            .0
        );
    }
    write(&completed, &value);
    write(&report, &json!({"changed":"secret-never-echo"}));
    assert!(
        !call(
            backend,
            "preflight-reconciliation",
            json!({"target":target})
        )
        .0
    );
    write(&target.join("reset-state/two.report.json"), &json!({}));
    assert!(
        !call(
            backend,
            "preflight-reconciliation",
            json!({"target":target})
        )
        .0
    );
}

#[test]
fn config_and_never_started_checks_refuse_outside_paths_and_existing_history() {
    let directory = workspace();
    let backend = directory.path();
    let config = backend.join("config");
    fs::create_dir(&config).unwrap();
    assert!(
        call(
            backend,
            "config-directory",
            json!({"request":{"config_dir":config}})
        )
        .0
    );
    assert!(
        !call(
            backend,
            "config-directory",
            json!({"request":{"config_dir":backend}})
        )
        .0
    );
    let runtime = backend.join(".local-tests/runtime");
    let identity = backend.join(".local-tests/identity");
    let selected = json!({"runtime_dir":runtime,"identity_ledger":identity});
    assert!(call(backend, "never-started", json!({"selected":selected})).0);
    assert!(!runtime.exists());
    fs::create_dir(&runtime).unwrap();
    assert!(call(backend, "never-started", json!({"selected":selected})).0);
    write(&runtime.join("producer-history.json"), &json!({}));
    assert!(!call(backend, "never-started", json!({"selected":selected})).0);
}
