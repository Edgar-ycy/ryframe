use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn public_native_driver_rejects_invalid_input_without_node_or_cargo_child() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__devex-runtime")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"{}").unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("runtime_protocol_input"));
}

#[test]
fn poisoned_project_environment_cannot_replace_the_stdin_protocol() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__devex-runtime")
        .env("RYFRAME_DEVEX_CACHE", "warm")
        .env("RYFRAME_DEVEX_CONCURRENCY", "10")
        .env(
            "RYFRAME_DEVEX_DRIVER_FINGERPRINT",
            format!("sha256:{}", "d".repeat(64)),
        )
        .env("APP_DATABASE_PASSWORD", "must-not-leak-from-old-env")
        .env("VITE_API_URL", "must-not-leak-from-old-env")
        .env("SNOWFLAKE_WORKER_ID", "must-not-leak-from-old-env")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"{}").unwrap();
    let result = child.wait_with_output().unwrap();
    assert_eq!(result.status.code(), Some(2));
    let diagnostics = String::from_utf8(result.stderr).unwrap();
    assert!(diagnostics.contains("runtime_protocol_input"));
    assert!(!diagnostics.contains("must-not-leak"));
    assert!(result.stdout.is_empty());
}

#[test]
fn valid_stdin_binding_is_unchanged_by_all_retired_runtime_variables() {
    let temp = tempfile::tempdir().unwrap();
    let fingerprint = |byte: char| format!("sha256:{}", byte.to_string().repeat(64));
    let input = serde_json::json!({
        "backend":temp.path(), "frontend":temp.path(), "runner_frontend":temp.path(),
        "driver":temp.path(), "output":temp.path().join("result.json"),
        "suite":"api", "cache":"warm", "concurrency":10,
        "input_sha256":"a".repeat(64), "driver_fingerprint":fingerprint('d'),
        "runner_frontend_fingerprint":fingerprint('e'),
        "source_fingerprints":{"backend":fingerprint('b'),"frontend":fingerprint('f'),"runner_frontend":fingerprint('e')},
        "native_tool":{"format_version":1,"kind":"ryframe-devex-native-tool",
            "path":temp.path().join("tool.exe"),"bytes":1,"sha256":"c".repeat(64)},
        "app_env":"test"
    });
    let run = |poisoned: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
        command
            .arg("__devex-runtime")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in [
            "RYFRAME_DEVEX_CONCURRENCY",
            "RYFRAME_DEVEX_CACHE",
            "RYFRAME_DEVEX_RUNTIME_INPUT_SHA256",
            "RYFRAME_DEVEX_DRIVER_FINGERPRINT",
            "RYFRAME_DEVEX_RUNNER_FRONTEND_FINGERPRINT",
            "RYFRAME_DEVEX_SOURCE_FINGERPRINTS",
            "RYFRAME_DEVEX_NATIVE_TOOL",
        ] {
            command.env_remove(key);
            if poisoned {
                command.env(key, "must-not-leak-from-old-env");
            }
        }
        if poisoned {
            command.env("APP_ENV", "prod");
        }
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&input).unwrap())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let clean = run(false);
    let poisoned = run(true);
    assert_eq!(clean.status.code(), Some(1));
    assert_eq!(clean.status.code(), poisoned.status.code());
    assert_eq!(clean.stdout, poisoned.stdout);
    assert_eq!(clean.stderr, poisoned.stderr);
    let diagnostics = String::from_utf8(clean.stderr).unwrap();
    assert!(diagnostics.contains("artifact_read"));
    assert!(!diagnostics.contains("must-not-leak"));
    assert!(!temp.path().join("result.json").exists());
}
