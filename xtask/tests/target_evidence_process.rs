//! 实际二进制的只读协议与脱敏退出行为，不启动共享服务。

use std::{
    io::Write,
    process::{Command, Stdio},
};

use serde_json::{Value, json};

fn invoke(arguments: &[&str], bytes: &[u8]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    command
        .args(arguments)
        .env("PATH", "")
        .env_remove("APP_ENV")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().unwrap();
    let _ = child.stdin.take().unwrap().write_all(bytes);
    child.wait_with_output().unwrap()
}

#[test]
fn evidence_protocol_refuses_invalid_input_without_echoing_credentials() {
    let secret = "fixture-password-never-echo";
    for input in [
        Vec::new(),
        vec![b'x'; 1024 * 1024 + 1],
        br#"{"operation":"history","operation":"reset-plan"}"#.to_vec(),
        serde_json::to_vec(&json!({"format_version":1,"operation":"reset-completed",
            "backend":".","target":".","manifest":{"password":secret},"sha":"bad"}))
        .unwrap(),
    ] {
        let output = invoke(&["__target-evidence"], &input);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stderr.is_empty());
        let text = std::str::from_utf8(&output.stdout).unwrap();
        assert!(!text.contains(secret));
        let value: Value = serde_json::from_str(text).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["remote_writes"], 0);
        assert_eq!(value["error"]["automatic_retry"], false);
    }
}

#[test]
fn evidence_protocol_does_not_accept_request_arguments() {
    let output = invoke(&["__target-evidence", "unexpected"], b"{}");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
}

#[test]
fn evidence_history_is_readonly_and_does_not_need_python() {
    let backend = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let parent = backend.join(".local-tests/target-evidence-process");
    std::fs::create_dir_all(&parent).unwrap();
    let directory = tempfile::tempdir_in(parent).unwrap();
    let mut names = vec![
        "initialize.started.json".to_string(),
        "prepare.json".into(),
        "request.json".into(),
        "reset-plan.json".into(),
        "reset-plan-confirm.json".into(),
    ];
    for stage in [
        "sentinel",
        "reset",
        "create-shared-control",
        "create-shared",
        "create-dedicated-a",
        "create-dedicated-b",
    ] {
        for state in ["intent", "confirmed"] {
            names.push(format!("{stage}.{state}.json"));
        }
    }
    for name in &names {
        std::fs::write(directory.path().join(name), b"{}\n").unwrap();
    }
    let request = json!({"format_version":1,"operation":"history",
        "backend":backend,"target":directory.path()});
    let output = invoke(
        &["__target-evidence"],
        &serde_json::to_vec(&request).unwrap(),
    );
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    let bindings = response["result"].as_object().unwrap();
    assert_eq!(bindings.len(), names.len());
    for name in names {
        assert_eq!(bindings[&name]["bytes"], 3);
        assert_eq!(std::fs::read(directory.path().join(name)).unwrap(), b"{}\n");
    }
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 17);
}

#[test]
fn verified_inventory_receipt_content_is_not_returned_to_stdout() {
    use sha2::{Digest, Sha256};
    let backend = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let parent = backend.join(".local-tests/target-evidence-process");
    std::fs::create_dir_all(&parent).unwrap();
    let directory = tempfile::tempdir_in(parent).unwrap();
    let inventory = directory.path().join("inventory-initial");
    std::fs::create_dir(&inventory).unwrap();
    let path = inventory.join("inventory.json");
    let secret = "fixture-private-receipt-content";
    let bytes =
        serde_json::to_vec(&json!({"inventories":{},"unexpected_private_field":secret})).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let request = json!({"format_version":1,"operation":"initial-inventory",
        "backend":backend,"target":directory.path(),"resumed":false,
        "receipt":{"path":path,"bytes":bytes.len(),"sha256":hex::encode(Sha256::digest(&bytes))}});
    let output = invoke(
        &["__target-evidence"],
        &serde_json::to_vec(&request).unwrap(),
    );
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result,
        json!({"ok":true,"result":{"status":"initial_inventory_verified","remote_writes":0}})
    );
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn migration_protocol_refuses_private_bad_output_and_binds_successful_operation() {
    let backend = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let parent = backend.join(".local-tests/target-evidence-process");
    std::fs::create_dir_all(&parent).unwrap();
    let directory = tempfile::tempdir_in(parent).unwrap();
    let request = json!({"format_version":1,"operation":"migration-operations",
        "backend":backend,"target":directory.path(),"executable":"fixture-migrate.exe"});
    let output = invoke(
        &["__target-evidence"],
        &serde_json::to_vec(&request).unwrap(),
    );
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let operations = value["result"]["operations"].clone();
    let path = directory.path().join(format!(
        "{}-{}.command.json",
        operations[0]["stage"].as_str().unwrap(),
        "0".repeat(32)
    ));
    let secret = "fixture-private-migration-output";
    let bytes = serde_json::to_vec(&json!({"command":operations[0]["command"],"returncode":0,
        "stdout":secret,"error_type":null,"stderr":""}))
    .unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let request = json!({"format_version":1,"operation":"migration-prefix",
        "backend":backend,"target":directory.path(),"operations":operations,"complete":false});
    let output = invoke(
        &["__target-evidence"],
        &serde_json::to_vec(&request).unwrap(),
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["error"]["remote_writes"], 0);
    assert_eq!(response["error"]["automatic_retry"], false);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let mut receipt: Value = serde_json::from_slice(&bytes).unwrap();
    receipt["stdout"] = request["operations"][0]["stdout"].clone();
    let bytes = serde_json::to_vec(&receipt).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let request = json!({"format_version":1,"operation":"migration-operation-record",
        "backend":backend,"target":directory.path(),"migration_operation":request["operations"][0]});
    let output = invoke(
        &["__target-evidence"],
        &serde_json::to_vec(&request).unwrap(),
    );
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["result"]["bytes"], bytes.len());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}
