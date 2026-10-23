//! 只读私有协议的真实进程边界，不启动数据库、缓存或对象服务。
use serde_json::{Value, json};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

fn invoke(payload: &[u8], extra: &[&str]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__fixture-service-history")
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(payload).unwrap();
    child.wait_with_output().unwrap()
}

fn request(operation: &str, arguments: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"format_version":1,"operation":operation,
        "backend":PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap(),"arguments":arguments})).unwrap()
}

#[test]
fn manifest_is_native_and_never_invokes_python() {
    let payload = request(
        "manifest",
        json!({"value":{"format_version":1,"kind":"reference-fixture-service-run",
        "review":{},"bootstrap":{},"execution_backend":"backend","scope_id":"fixture","data_directory_was_empty":true}}),
    );
    let output = invoke(&payload, &[]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"ok":true,"result":null})
    );
}

#[test]
fn unknown_fields_duplicates_and_cli_arguments_fail_with_private_diagnostics() {
    for (payload, extra) in [
        (request("credential-marker", json!({})), Vec::new()),
        (
            br#"{"format_version":1,"format_version":2,"secret":"credential-marker"}"#.to_vec(),
            Vec::new(),
        ),
        (
            request("manifest", json!({"value":{"secret":"credential-marker"}})),
            vec!["--write"],
        ),
    ] {
        let output = invoke(&payload, &extra);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stderr.is_empty());
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["ok"], false);
        assert!(!String::from_utf8_lossy(&output.stdout).contains("credential-marker"));
    }
}

#[test]
fn history_cannot_open_an_unowned_directory() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let output = invoke(
        &request(
            "history",
            json!({"run":root,"state":{"attempts":[]},"ready":false,"settled":false}),
        ),
        &[],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    assert!(!root.join("state.json").exists());
}

#[test]
fn initial_prefix_and_pending_failures_do_not_require_service_processes() {
    let attempts = [
        ("storage-target", "initial"),
        ("cache-target", "initial"),
        ("fixture-buckets", "prepare"),
    ]
    .map(|(stage, mode)| json!({"stage":stage,"mode":mode,"status":"passed"}));
    let output = invoke(
        &request("initial", json!({"state":{"attempts":attempts}})),
        &[],
    );
    assert!(output.status.success());
    let output = invoke(
        &request(
            "reconcilable-close",
            json!({"state":{"attempts":[{"number":1,"stage":"fixture-services","mode":"close","status":"failed"}]},
        "history":{"closed":false,"external_recovery":null,"unsettled":[1]}}),
        ),
        &[],
    );
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["result"]["number"], 1);
}
