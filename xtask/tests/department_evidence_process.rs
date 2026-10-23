//! 部门证据协议的真实进程成功、失败与取消；不访问共享资源。
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

struct Fixture {
    run: PathBuf,
    output: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let backend = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let run = backend.join(".local-tests").join(format!(
            "department-process-中文 空格-{}",
            uuid::Uuid::new_v4()
        ));
        let output = run.join("seed-runtime/attempt-0001");
        fs::create_dir_all(&output).unwrap();
        fs::write(output.join(format!("command-{}.json", "1".repeat(32))), serde_json::to_vec(&json!({
            "command":["cargo","-V"],"returncode":0,"error_type":null,"stdout":"cargo fixture","stderr":""
        })).unwrap()).unwrap();
        Self { run, output }
    }
    fn payload(&self, operation: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({"format_version":1,"kind":"ryframe-seed-evidence","run_dir":self.run,
            "operation":"department-guard","arguments":{"backend_root":self.run.parent().unwrap().parent().unwrap(),
            "parameters":{"operation":operation,"output":self.output,"target":null,"attempt":1}}})).unwrap()
    }
    fn invoke(&self, payload: &[u8]) -> std::process::Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .arg("__seed-evidence")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(payload).unwrap();
        child.wait_with_output().unwrap()
    }

    fn model_payload(&self, method: &str, descriptor: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"format_version":1,"kind":"ryframe-seed-evidence","run_dir":self.run,
            "operation":"department-guard","arguments":{"backend_root":self.run.parent().unwrap().parent().unwrap(),
                "parameters":{"operation":"model","method":method,"input":descriptor}}})).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.run).unwrap();
    }
}

#[test]
fn real_protocol_publishes_only_closed_guard_and_refuses_unknown_write_evidence() {
    let fixture = Fixture::new();
    let output = fixture.invoke(&fixture.payload("publish"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], true);
    let manifest = fs::read(fixture.output.join("adoption-guard.json")).unwrap();
    fs::write(fixture.output.join("session-launch.json"), "unknown-write").unwrap();
    let rejected = fixture.invoke(&fixture.payload("inspect"));
    assert_eq!(rejected.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&rejected.stdout).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["automatic_retry_permitted"], false);
    assert_eq!(
        fs::read(fixture.output.join("adoption-guard.json")).unwrap(),
        manifest
    );
    assert!(rejected.stderr.is_empty());
}

#[test]
fn real_protocol_rejects_truncated_duplicate_and_extra_arguments_without_evidence() {
    let fixture = Fixture::new();
    for payload in [
        b"{\"password\":\"secret-credential\"".as_slice(),
        b"{\"format_version\":1,\"format_version\":1}".as_slice(),
    ] {
        let output = fixture.invoke(payload);
        assert_eq!(output.status.code(), Some(1));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("secret-credential"));
        assert!(output.stderr.is_empty());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["__seed-evidence", "extra"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(!fixture.output.join("adoption-guard.json").exists());
}

#[test]
fn cancelling_real_protocol_while_waiting_for_input_preserves_unpublished_evidence() {
    let fixture = Fixture::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__seed-evidence")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"{\"format_version\":")
        .unwrap();
    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success());
    assert!(!fixture.output.join("adoption-guard.json").exists());
    assert_eq!(fs::read_dir(&fixture.output).unwrap().count(), 1);
}

fn input_binding(path: &std::path::Path) -> Value {
    use sha2::{Digest, Sha256};
    let content = fs::read(path).unwrap();
    json!({"path":path,"bytes":content.len(),"sha256":hex::encode(Sha256::digest(&content))})
}

#[test]
fn model_protocol_reads_large_bound_input_without_extending_stdin_limit_or_writing_receipts() {
    let fixture = Fixture::new();
    let path = fixture.run.join("model-input.json");
    let page = json!({"response":{"code":200,"ignored": "x".repeat(100000),"data":{
        "items":[],"page":1,"page_size":100,"max_page_size":100,"total":0,"total_pages":0}}});
    fs::write(&path, serde_json::to_vec(&page).unwrap()).unwrap();
    let payload = fixture.model_payload("page-data", input_binding(&path));
    assert!(payload.len() < 65536);
    let before = fs::read(&path).unwrap();
    let output = fixture.invoke(&payload);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["result"], page["response"]["data"]);
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read_dir(&fixture.run).unwrap().count(), 2);
    fs::write(&path, b"{\"private-password\":\"must-not-appear\"}").unwrap();
    let rejected = fixture.invoke(&payload);
    assert_eq!(rejected.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&rejected.stdout).contains("must-not-appear"));
    assert!(rejected.stderr.is_empty());
}

#[test]
fn model_protocol_refuses_duplicate_unknown_fields_and_path_escape_without_retry() {
    let fixture = Fixture::new();
    let path = fixture.run.join("model-input.json");
    for content in [
        b"{\"response\":{},\"response\":{}}".as_slice(),
        b"{\"response\":{},\"unknown\":true}".as_slice(),
    ] {
        fs::write(&path, content).unwrap();
        let output = fixture.invoke(&fixture.model_payload("page-data", input_binding(&path)));
        assert_eq!(output.status.code(), Some(1));
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["error"]["automatic_retry_permitted"], false);
    }
    let mut binding = input_binding(&path);
    binding["path"] = json!(fixture.run.join("../model-input.json"));
    let output = fixture.invoke(&fixture.model_payload("page-data", binding));
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fs::read_dir(&fixture.output).unwrap().count(), 1);
}

#[cfg(windows)]
#[test]
fn model_protocol_detects_hardlinked_input_drift_without_publishing_completion() {
    let fixture = Fixture::new();
    let path = fixture.run.join("model-input.json");
    let mut request = json!({"response":{"code":200,"tag":"original","data":{
        "items":[],"page":1,"page_size":100,"max_page_size":100,"total":0,"total_pages":0}}});
    fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
    let payload = fixture.model_payload("page-data", input_binding(&path));
    let alias = fixture.run.join("model-input-alias.json");
    fs::hard_link(&path, &alias).unwrap();
    request["response"]["tag"] = json!("changed");
    fs::write(&alias, serde_json::to_vec(&request).unwrap()).unwrap();
    let expected = fs::read(&path).unwrap();
    let output = fixture.invoke(&payload);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fs::read(&path).unwrap(), expected);
    assert!(
        fixture
            .invoke(&fixture.model_payload("page-data", input_binding(&path)))
            .status
            .success()
    );
    assert!(!fixture.output.join("adoption-guard.json").exists());
    assert_eq!(fs::read_dir(&fixture.output).unwrap().count(), 1);
}

#[cfg(windows)]
#[test]
fn real_protocol_rejects_junction_evidence_without_touching_link_target() {
    let fixture = Fixture::new();
    let target = fixture.run.join("preserved-target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("preserve.txt"), b"unchanged").unwrap();
    let junction = fixture.output.join("unknown-junction");
    let output = Command::new("cmd.exe")
        .args(["/c", "mklink", "/J"])
        .arg(&junction)
        .arg(&target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result = fixture.invoke(&fixture.payload("inspect"));
    fs::remove_dir(&junction).unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(fs::read(target.join("preserve.txt")).unwrap(), b"unchanged");
    assert!(!fixture.output.join("adoption-guard.json").exists());
}
