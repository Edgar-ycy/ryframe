//! 生命周期私有进程协议：只检查本机文件，拒绝非法请求，不访问共享服务。
use serde_json::{Value, json};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

fn invoke(payload: &[u8], extra: &[&str]) -> std::process::Output {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .current_dir(root)
        .arg("__fixture-service-lifecycle")
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(payload).unwrap();
    child.wait_with_output().unwrap()
}

fn request(operation: &str, write: bool, owner: Value) -> Value {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    json!({"format_version":1,"backend":root,"operation":operation,
        "review":root.join(".local-tests/missing-review.json"),
        "environment":root.join(".local-tests/missing-environment.json"),"owner":owner,"write":write})
}

#[test]
fn explicit_write_and_owner_are_checked_before_opening_context() {
    for (operation, write, owner, expected) in [
        ("close", false, Value::Null, "--write"),
        ("status", true, Value::Null, "只读"),
        ("status", false, json!("credential-marker"), "owner"),
        ("restart", true, Value::Null, "owner"),
        ("close", true, json!("credential-marker"), "owner"),
        ("credential-marker", true, Value::Null, "未知"),
    ] {
        let output = invoke(
            &serde_json::to_vec(&request(operation, write, owner)).unwrap(),
            &[],
        );
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stderr.is_empty());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["code"], "invalid_fixture_lifecycle");
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains(expected)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("credential-marker"));
    }
}

#[test]
fn duplicate_unknown_oversized_or_argv_payloads_cannot_execute() {
    let mut unknown = request("close", true, Value::Null);
    unknown["password"] = json!("credential-marker");
    for (payload, extra) in [
        (serde_json::to_vec(&unknown).unwrap(), vec![]),
        (
            br#"{"format_version":1,"format_version":1,"secret":"credential-marker"}"#.to_vec(),
            vec![],
        ),
        (vec![b'x'; 4 * 1024 * 1024 + 1], vec![]),
        (
            serde_json::to_vec(&request("close", true, Value::Null)).unwrap(),
            vec!["--write"],
        ),
    ] {
        let output = invoke(&payload, &extra);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stderr.is_empty());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("credential-marker"));
    }
}

#[test]
fn escaped_context_is_rejected_without_creating_run_files() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let mut payload = request("close", true, Value::Null);
    payload["review"] = json!(root.join("Cargo.toml"));
    let output = invoke(&serde_json::to_vec(&payload).unwrap(), &[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    assert!(!root.join("state.json").exists());
    assert!(!root.join("run.lock").exists());
}
