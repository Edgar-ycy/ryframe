//! 复制后处理私有 stdin 协议的真实进程边界。
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

fn invoke(raw: &[u8], extra: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__clone-post-process")
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(raw).unwrap();
    child.wait_with_output().unwrap()
}
fn call(operation: &str, payload: Value) -> Output {
    invoke(
        &serde_json::to_vec(&json!({"operation":operation,"payload":payload})).unwrap(),
        &[],
    )
}
fn rejected(output: Output) {
    assert_eq!(output.status.code(), Some(1));
    let raw = String::from_utf8(output.stdout).unwrap();
    assert!(!raw.contains("Private-Secret"));
    let value: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value.as_object().unwrap().len(), 2)
}

#[test]
fn command_plan_uses_bound_tool_and_never_serializes_credentials() {
    let backend = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let tool = PathBuf::from(env!("CARGO_BIN_EXE_xtask"));
    let bytes = std::fs::read(&tool).unwrap();
    let output = call(
        "command",
        json!({"backend":backend,"run_dir":backend.join(".local-tests/run"),"attempt":4,"kind":"quota-apply","output":backend.join(".local-tests/run/seed-runtime/attempt-0004"),"tool":{"path":tool,"sha256":hex::encode(Sha256::digest(bytes))},"registration":{"identity_plan":{"path":backend.join(".local-tests/plan.json"),"sha256":"a".repeat(64)},"password":"Private-Secret"},"configuration":null}),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let raw = String::from_utf8(output.stdout).unwrap();
    assert!(!raw.contains("Private-Secret"));
    let value: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["ok"], true);
    assert!(
        value["result"]["argv"]
            .as_array()
            .unwrap()
            .contains(&json!("--quota-mode"))
    );
}

#[test]
fn retired_shortcuts_are_rejected_as_unknown_operations() {
    for operation in ["recover-identity-lock", "runtime-proof"] {
        let output = call(operation, Value::Null);
        assert_eq!(output.status.code(), Some(1), "{operation}");
        assert!(output.stderr.is_empty(), "{operation}");
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            response,
            json!({"ok":false,"error_type":"PostProcessError"}),
            "{operation}"
        );
    }
}

#[test]
fn duplicate_invalid_oversized_unknown_and_argument_requests_fail_closed() {
    for raw in [Vec::new(),br#"{"operation":"command","operation":"runtime-proof","payload":{"password":"Private-Secret"}}"#.to_vec(),vec![0xff],vec![b' ';1024*1024+1],serde_json::to_vec(&json!({"operation":"unknown","payload":{"password":"Private-Secret"}})).unwrap()]{rejected(invoke(&raw,&[]));}
    rejected(invoke(b"", &["Private-Secret"]));
}
