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
