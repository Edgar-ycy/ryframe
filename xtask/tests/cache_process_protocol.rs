//! 缓存控制私有入口的真实进程核验；不启动 Redis 或访问共享恢复资源。
use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Output, Stdio},
};

fn invoke(raw: &[u8], arguments: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__cache-process")
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(raw).unwrap();
    child.wait_with_output().unwrap()
}

fn request(operation: &str, arguments: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "format_version":1,"kind":"ryframe-cache-process","operation":operation,"arguments":arguments
    }))
    .unwrap()
}

fn rejected(output: Output) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let raw = String::from_utf8(output.stdout).unwrap();
    assert!(!raw.contains("private-secret-marker"));
    let value: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"], "CacheProcessRejected");
    assert_eq!(value["exit_code"], Value::Null);
    assert_eq!(value.as_object().unwrap().len(), 4);
    assert!(value["message"].as_str().unwrap().contains("禁止自动重放"));
}

#[test]
fn dispatch_accepts_direct_password_in_stdin_without_echoing_it() {
    let output = invoke(
        &request("password", json!({"value":"private-secret-marker"})),
        &[],
    );
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"ok":true,"result":null})
    );
    rejected(invoke(
        &request("password", json!({"value":"\0private-secret-marker"})),
        &[],
    ));
}

#[test]
fn malformed_duplicate_oversized_and_argument_bearing_requests_fail_closed() {
    for raw in [
        Vec::new(),
        b"{\"format_version\":1,\"format_version\":1,\"password\":\"private-secret-marker\"}"
            .to_vec(),
        b"{} {}".to_vec(),
        vec![0xff],
        vec![b' '; 1024 * 1024 + 1],
        request("unknown", json!({"password":"private-secret-marker"})),
        request(
            "password",
            json!({"value":"valid","password_env":"APP_REDIS_PASSWORD"}),
        ),
    ] {
        rejected(invoke(&raw, &[]));
    }
    rejected(invoke(b"", &["private-secret-marker"]));
}

#[test]
fn facts_guard_requires_exact_configuration_and_run_generation() {
    use sha2::{Digest, Sha256};
    let configuration = json!({"bind":"127.0.0.1","port":"16390","dir":"/mnt/d/data",
        "databases":"1","protected-mode":"yes","save":"","appendonly":"no","daemonize":"no"});
    let mut arguments = json!({"request":{"port":16390,"directory":{"path":"D:/data"},
        "configuration":{"path":"D:/data/redis.conf"},"previous_run_id":"a".repeat(40)},
        "identity":{"pid":300},"facts":{"process_id":"300","run_id":"b".repeat(40),
            "config_file":"/mnt/d/data/redis.conf","configuration_sha256":
                hex::encode(Sha256::digest(serde_json::to_vec(&configuration).unwrap()))},"run_id":null});
    let output = invoke(&request("check-facts", arguments.clone()), &[]);
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"ok":true,"result":null})
    );
    arguments["run_id"] = json!("a".repeat(40));
    rejected(invoke(&request("check-facts", arguments.clone()), &[]));
    arguments["run_id"] = Value::Null;
    arguments["facts"]["configuration_sha256"] = json!("private-secret-marker");
    rejected(invoke(&request("check-facts", arguments), &[]));
}
