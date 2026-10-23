//! 仅测试私有进程协议，不连接业务服务或共享验收资源。
use std::{
    io::Write,
    process::{Command, Stdio},
};

use serde_json::{Value, json};

fn invoke(raw: &[u8]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__source-evidence")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(raw).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn runtime_protocol_rejects_unbounded_duplicate_malformed_and_secret_bearing_requests() {
    let mut requests = vec![
        b"{\"operation\":\"runtime-cache-receipt\",\"write\":false,\"arguments\":{\"value\":\"private-secret-marker\"}}".to_vec(),
        b"{\"operation\":\"producer-ready\",\"write\":false,\"arguments\":{\"raw\":\"private-secret-marker\",\"operation\":\"a\",\"start\":{},\"lineage\":{},\"staging\":{}}}".to_vec(),
        b"{\"operation\":\"producer-inputs\",\"write\":false,\"write\":false,\"arguments\":{}}".to_vec(),
        b"{\"operation\":\"runtime-cache-resume\",\"write\":true,\"arguments\":{}}".to_vec(),
    ];
    requests.push(vec![b'x'; 16 * 1024 * 1024 + 1]);
    requests.push(
        serde_json::to_vec(&json!({"operation":"runtime-manifest","write":false,
        "arguments":{"backend":"private-secret-marker","directory":"private-secret-marker"}}))
        .unwrap(),
    );
    for raw in requests {
        let output = invoke(&raw);
        assert!(!output.status.success());
        assert!(output.stderr.is_empty());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private-secret-marker"));
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["automatic_retry_permitted"], false);
        assert_eq!(response["error"]["remote_write_may_have_occurred"], false);
    }
}

#[test]
fn ready_must_bind_operation_start_tool_contract_lineage_and_one_bounded_line() {
    let modules = json!([{"path":"native-tool","sha256":"a".repeat(64)}]);
    use sha2::{Digest, Sha256};
    let ready = json!({"format_version":1,"kind":"restore-source-producer-ready","operation_id":"b".repeat(32),
        "source_generation_sha256":"c".repeat(64),"staging_manifest_sha256":"d".repeat(64),
        "runtime_files_sha256":hex::encode(Sha256::digest(serde_json::to_vec(&modules).unwrap())),
        "contract_file_sha256":"e".repeat(64),"lineage_file_sha256":"f".repeat(64)});
    let mut request = json!({"operation":"producer-ready","write":false,"arguments":{
        "raw":format!("{ready}\n"),"operation":"b".repeat(32),"start":{"sha256":"c".repeat(64)},
        "lineage":{"sha256":"f".repeat(64)},"staging":{"descriptor":{"sha256":"d".repeat(64)},
            "manifest":{"files":[{"path":"contract","sha256":"e".repeat(64)}],"contract":"contract","runtime_modules":modules}}}});
    let output = invoke(&serde_json::to_vec(&request).unwrap());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    for raw in [
        format!("{ready}"),
        format!("{ready}\n{ready}\n"),
        "x".repeat(65537),
    ] {
        request["arguments"]["raw"] = json!(raw);
        assert!(
            !invoke(&serde_json::to_vec(&request).unwrap())
                .status
                .success()
        );
    }
    request["arguments"]["raw"] = json!(format!("{ready}\n"));
    request["arguments"]["lineage"]["sha256"] = json!("0".repeat(64));
    assert!(
        !invoke(&serde_json::to_vec(&request).unwrap())
            .status
            .success()
    );
}
