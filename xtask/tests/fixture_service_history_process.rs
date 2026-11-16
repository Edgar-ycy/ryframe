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

fn test_directory() -> tempfile::TempDir {
    let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".local-tests");
    std::fs::create_dir_all(&parent).unwrap();
    tempfile::tempdir_in(parent).unwrap()
}

#[test]
fn unknown_fields_duplicates_and_cli_arguments_fail_with_private_diagnostics() {
    let run = test_directory();
    let history = request(
        "history",
        json!({"run":run.path(),"state":{"attempts":[]},"ready":false,"settled":false}),
    );
    assert!(invoke(&history, &[]).status.success());

    for (payload, extra) in [
        (request("credential-marker", json!({})), Vec::new()),
        (
            br#"{"format_version":1,"format_version":2,"secret":"credential-marker"}"#.to_vec(),
            Vec::new(),
        ),
        (history, vec!["--write", "credential-marker"]),
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
fn service_registration_and_observation_reject_escaped_or_unknown_inputs_privately() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    for (operation, arguments) in [
        (
            "service-registered",
            json!({"value":{"backend":root,"run":root},"settled":true}),
        ),
        (
            "service-observe",
            json!({"services":{"tree":{"runtime_directory":root}}}),
        ),
        (
            "service-registered",
            json!({"value":{},"settled":true,"secret":"credential-marker"}),
        ),
        (
            "service-observe",
            json!({"services":{},"secret":"credential-marker"}),
        ),
    ] {
        let output = invoke(&request(operation, arguments), &[]);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stderr.is_empty());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["code"], "invalid_fixture_history");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("credential-marker"));
    }
    assert!(!root.join("state.json").exists());
    assert!(!root.join("run.lock").exists());
}

#[test]
fn service_path_projection_runs_without_python_and_rejects_unknown_fields() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let review = json!({"future_root":root.join(".local-tests/reference-fixture/future"),"scopes":{"seed":{"backend_dir":root}}});
    let output = invoke(&request("service-run", json!({"review":review})), &[]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let response = serde_json::from_slice::<Value>(&output.stdout).unwrap();
    assert_eq!(
        PathBuf::from(response["result"].as_str().unwrap()),
        root.join(".local-tests/reference-fixture/future/service-run")
    );
    let output = invoke(
        &request(
            "service-run",
            json!({"review":review,"private":"private-marker"}),
        ),
        &[],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private-marker"));
}

#[test]
fn private_service_response_limit_never_prints_large_credentials() {
    let directory = test_directory();
    let path = directory.path().join("large-private.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({"secret":"private-marker".repeat(300000)})).unwrap(),
    )
    .unwrap();
    let output = invoke(&request("service-document", json!({"path":path})), &[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private-marker"));
}
