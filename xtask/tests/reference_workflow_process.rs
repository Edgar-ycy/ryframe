use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

use serde_json::{Value, json};

fn invoke(raw: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__source-evidence")
        .env("APP_ENV", "prod")
        .env("RYFRAME_XTASK_RECOVERY_REFERENCE", "ignored-private-secret")
        .env("APP_DATABASE_PASSWORD", "ignored-private-secret")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(raw).unwrap();
    child.wait_with_output().unwrap()
}

fn request(operation: &str, write: bool, args: &Value) -> Output {
    invoke(
        &serde_json::to_vec(&json!({"operation":operation,"write":write,"arguments":args}))
            .unwrap(),
    )
}

fn temporary() -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".local-tests/reference-workflow-process");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

#[test]
fn invalid_protocol_is_sanitized_and_cannot_authorize_writes_or_retries() {
    for raw in [b"\xff".as_slice(), br#"{"operation":"reference-request","write":false,"arguments":{"raw":"{\"private-secret\":1,\"private-secret\":2}"}}"#,
        br#"{"operation":"reference-request","write":false,"arguments":{"raw":"private-secret"}}"#,
        br#"{"operation":"reference-receipt-start","write":false,"arguments":{"private-secret":true}}"#] {
        let output = invoke(raw);
        assert!(!output.status.success());
        assert!(output.stderr.is_empty());
        let encoded = String::from_utf8(output.stdout).unwrap();
        assert!(!encoded.contains("private-secret"));
        let value: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["remote_write_may_have_occurred"], false);
        assert_eq!(value["error"]["automatic_retry_permitted"], false);
    }
}

#[test]
fn read_only_owner_verification_has_no_filesystem_side_effects() {
    let directory = temporary();
    let work = directory.path().join("未创建 目录");
    let args = json!({"plan":{"id":"reference-case","work_dir":work}});
    assert!(
        !request("reference-work-verify", false, &args)
            .status
            .success()
    );
    assert!(!work.exists());
    assert!(
        !request("reference-work-directory", false, &args)
            .status
            .success()
    );
    assert!(!work.exists());
    let created = request("reference-work-directory", true, &args);
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stdout)
    );
    let before = fs::read(work.join("reference-owner.json")).unwrap();
    assert!(
        request("reference-work-verify", false, &args)
            .status
            .success()
    );
    assert_eq!(fs::read(work.join("reference-owner.json")).unwrap(), before);
    assert_eq!(fs::read_dir(work).unwrap().count(), 1);
}

#[test]
fn receipt_terminal_publish_requires_explicit_write_and_keeps_unknown_failure() {
    let directory = temporary();
    let plan = json!({"id":"reference-case","work_dir":directory.path().join("work")});
    assert!(
        request("reference-work-directory", true, &json!({"plan":plan}))
            .status
            .success()
    );
    let started = request(
        "reference-receipt-start",
        true,
        &json!({"plan":plan,"command":"dataset","copy_id":null,"root":null,"receipt":null,"bindings":null}),
    );
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stdout)
    );
    let started: Value = serde_json::from_slice(&started.stdout).unwrap();
    let receipt = &started["result"]["value"];
    let args = json!({"receipt":receipt,"result":null,"failure":{"error_type":"TimeoutExpired","message":"未知写入禁止重放"},"resource_images":{"before":{"verified":true}}});
    assert!(
        !request("reference-receipt-finish", false, &args)
            .status
            .success()
    );
    let path = receipt["path"].as_str().unwrap();
    let running: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(running["status"], "running");
    let finished = request("reference-receipt-finish", true, &args);
    assert!(
        finished.status.success(),
        "{}",
        String::from_utf8_lossy(&finished.stdout)
    );
    let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(value["status"], "failed");
    assert!(value.get("result").is_none());
    assert_eq!(value["resource_images"]["before"]["verified"], true);
    assert!(
        !request("reference-receipt-finish", true, &args)
            .status
            .success()
    );
}
