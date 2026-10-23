//! Worker 有界守卫的真实进程协议；不启动产品或操作共享运行资源。
use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

fn invoke(bytes: &[u8], args: &[&str]) -> (bool, Value) {
    let mut process = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__full-stack-worker")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    process.stdin.take().unwrap().write_all(bytes).unwrap();
    let output = process.wait_with_output().unwrap();
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private-do-not-echo"));
    (
        output.status.success(),
        serde_json::from_slice(&output.stdout).unwrap(),
    )
}

#[test]
fn malformed_unknown_and_oversized_protocols_fail_without_exposing_input() {
    for bytes in [
        br#"{"private-do-not-echo":"credential"}"#.as_slice(),
        br#"{"format_version":1,"format_version":2}"#,
        b"[]",
        b"{}{}",
        b"\xff",
    ] {
        let (ok, result) = invoke(bytes, &[]);
        assert!(!ok);
        assert_eq!(result["error"], "WorkerRejected");
    }
    assert!(!invoke(&vec![b' '; 1024 * 1024 + 1], &[]).0);
    assert!(!invoke(b"{}", &["--request-file", "private-do-not-echo"]).0);
}

#[test]
fn probe_request_has_closed_fields_and_does_not_echo_invalid_credentials() {
    let request = json!({"format_version":1,"kind":"ryframe-full-stack-worker","operation":"probe",
        "arguments":{"action":"ready","url":"http://user:private-do-not-echo@127.0.0.1:1/readyz","timeout":0}});
    let (ok, result) = invoke(&serde_json::to_vec(&request).unwrap(), &[]);
    assert!(!ok);
    assert_eq!(result["ok"], false);
    assert!(result["message"].as_str().unwrap().contains("凭据"));
}

#[test]
fn cancellation_before_complete_input_never_publishes_success_or_starts_child() {
    let mut process = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__full-stack-worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    process
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"{\"format_version\":")
        .unwrap();
    thread::sleep(Duration::from_millis(100));
    assert!(process.try_wait().unwrap().is_none());
    process.kill().unwrap();
    let output = process.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}
