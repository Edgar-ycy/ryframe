//! 夹具环境原生边界的真实 Windows/Unix 进程协议，不连接外部服务。
use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn invoke(bytes: &[u8], extra: &[&str]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__fixture-environment")
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    child.wait_with_output().unwrap()
}
fn request(operation: &str, arguments: Value) -> Vec<u8> {
    serde_json::to_vec(
        &json!({"format_version":1,"operation":operation,"arguments":arguments,"write":false}),
    )
    .unwrap()
}
#[test]
fn valid_guard_uses_native_protocol_without_child_tools() {
    let output = invoke(
        &request("reset-password", json!({"admin_password":"Admin1!fixture"})),
        &[],
    );
    assert!(output.status.success(), "{:?}", output);
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"ok":true,"result":{}})
    );
}
#[test]
fn malformed_duplicate_and_extra_arguments_fail_privately() {
    for (bytes, extra) in [
        (
            request(
                "reset-password",
                json!({"admin_password":"private-credential-marker"}),
            ),
            vec![],
        ),
        (
            br#"{"format_version":1,"format_version":2,"secret":"private-credential-marker"}"#
                .to_vec(),
            vec![],
        ),
        (
            request("fixture", json!({"secret":"private-credential-marker"})),
            vec!["--write"],
        ),
        (vec![0xff, 0xfe], vec![]),
    ] {
        let output = invoke(&bytes, &extra);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stderr.is_empty());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["automatic_retry_permitted"], false);
        assert_eq!(value["error"]["remote_write_may_have_occurred"], false);
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private-credential-marker"));
    }
}
#[test]
fn cancellation_during_unfinished_input_publishes_no_response() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__fixture-environment")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(br#"{"format_version":1,"arguments":{"admin_password":"private-marker"}"#)
        .unwrap();
    child.kill().unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}
