//! fresh CLI 实际受管协议：无 Python、无项目配置环境覆盖、无写入与凭据回显。

use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

use serde_json::{Value, json};

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
