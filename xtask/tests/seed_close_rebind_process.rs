//! seed close/rebind 守卫的真实进程：文件绑定、取消、未知结果与错误脱敏。
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

struct Fixture {
    run: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let backend = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let run = backend.join(".local-tests").join(format!(
            "seed-close-rebind-中文 空格-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&run).unwrap();
        Self { run }
    }
    fn write(&self, name: &str, value: &Value) {
        fs::write(self.run.join(name), serde_json::to_vec(value).unwrap()).unwrap();
    }
    fn binding(&self, name: &str) -> Value {
        let path = self.run.join(name);
        let bytes = fs::read(&path).unwrap();
        json!({"path":path,"bytes":bytes.len(),"sha256":hex::encode(Sha256::digest(&bytes))})
    }
    fn request(&self, operation: &str, arguments: Value) -> Vec<u8> {
        self.write(
            "parameters.json",
            &json!({"operation":operation,"arguments":arguments}),
        );
        serde_json::to_vec(&json!({"format_version":1,"kind":"ryframe-seed-evidence","run_dir":self.run,
            "operation":"close-rebind-guard","arguments":{"backend_root":Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap(),
                "parameters":self.binding("parameters.json")}})).unwrap()
    }
    fn files(&self) -> Vec<(String, Vec<u8>)> {
        let mut files = fs::read_dir(&self.run)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().to_str().unwrap().to_owned(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        files.sort();
        files
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.run).unwrap();
    }
}
fn invoke(bytes: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__seed-evidence")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    child.wait_with_output().unwrap()
}
fn reject(output: &Output) {
    assert_eq!(output.status.code(), Some(1));
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["automatic_retry_permitted"], false);
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("secret-credential"));
}
fn hash(value: &Value) -> String {
    hex::encode(Sha256::digest(serde_json::to_vec(value).unwrap()))
}

#[test]
fn bound_rebind_transition_is_read_only_and_rejects_endpoint_and_unknown_operations() {
    let fixture = Fixture::new();
    let old = json!({"attempt":7,"request":{},"data_directory":{},"api_url":"http://127.0.0.1:29000",
        "console_url":"http://127.0.0.1:29001","storage":{"sha256":"fixed","identity":{"executable":"rustfs.exe"}}});
    let mut current = old.clone();
    current["attempt"] = json!(53);
    let request = fixture.request(
        "rebind-transition",
        json!({"original":old,"current":current}),
    );
    let before = fixture.files();
    let output = invoke(&request);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        response["result"],
        json!({"value":null,"remote_writes":0,"automatic_retry_permitted":false})
    );
    assert_eq!(before, fixture.files());
    current["api_url"] = json!("secret-credential");
    reject(&invoke(&fixture.request(
        "rebind-transition",
        json!({"original":old,"current":current}),
    )));
    reject(&invoke(
        &fixture.request("unknown", json!({"password":"secret-credential"})),
    ));
}

#[test]
fn stale_parameter_binding_and_duplicate_raw_json_fail_without_writing_evidence() {
    let fixture = Fixture::new();
    let request = fixture.request(
        "identity",
        json!({"value":{"pid":101,"started":"123","executable":"api.exe"}}),
    );
    fixture.write("parameters.json", &json!({"password":"secret-credential"}));
    let before = fixture.files();
    reject(&invoke(&request));
    assert_eq!(before, fixture.files());
    reject(&invoke(&fixture.request(
        "parse-snapshot",
        json!({"raw":"{\"active\":{},\"active\":{\"secret-credential\":1}}"}),
    )));
}

#[test]
fn incomplete_stdin_cancellation_publishes_no_result_and_preserves_input() {
    let fixture = Fixture::new();
    let request = fixture.request("identity", json!({"value":{}}));
    let before = fixture.files();
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
        .write_all(&request[..request.len() / 2])
        .unwrap();
    child.kill().unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(before, fixture.files());
}

#[test]
fn checkpoint_process_binds_original_attempt_controller_manifest_and_actual_handoff() {
    let fixture = Fixture::new();
    let handoff = json!({"scope_id":"seed-close"});
    fixture.write("handoff.json", &handoff);
    fixture.write("manifest.json", &json!({"fixture":true}));
    let attempt = json!({"number":1,"stage":"seed-runtime","mode":"close","status":"running",
        "started_at":"2026-10-23T00:00:00.000Z","finished_at":null,"sources":{},"result":null,"error_type":null});
    let identity = json!({"pid":101,"started":"123","executable":"api.exe"});
    let controller = json!({"format_version":1,"kind":"devex-stage-controller","attempt":1,"attempt_sha256":hash(&attempt),
        "owner":{"format_version":1,"identity":identity,"directory":fixture.run,"manifest_sha256":fixture.binding("manifest.json")["sha256"]}});
    fixture.write("controller-0001.json", &controller);
    let control = json!({"format_version":1,"kind":"devex-clone-runtime-control","operation":"status","scope_id":"seed-close",
        "runtime_directory":fixture.run,"producer_history":fixture.run.join("producer-history.json"),
        "processes":{"api":{"state":"running","ready":true,"identity":identity},
            "worker":{"state":"running","ready":true,"identity":identity}}});
    let result = json!({"format_version":1,"kind":"devex-clone-seed-close","status":"needs_reconciliation","phase":"running",
        "handoff":fixture.binding("handoff.json"),"runtime_directory":fixture.run,"scope_id":"seed-close",
        "process_identities":{"api":identity,"worker":identity},"runtime_status":control,"api_stop":null,"drain":null,
        "worker_stop":null,"final_snapshot":null,"last_snapshot":null,"terminal_non_success":null,"stopped_evidence":null,
        "worker_preserved":true,"reason":null,"error_type":null,"outbox_drained":false,"direct_remote_writes":0,
        "worker_drain_writes":true,"restore_qualified":false});
    let mut checkpoint = json!({"format_version":1,"kind":"devex-clone-seed-close-checkpoint","attempt":1,
        "controller":fixture.binding("controller-0001.json"),"handoff":fixture.binding("handoff.json"),
        "runtime_directory":fixture.run,"scope_id":"seed-close","phase":"running","previous":null,"resumed_from":null,"result":result});
    checkpoint["sha256"] = json!(hash(&checkpoint));
    let input = json!({"directory":fixture.run,"attempt":attempt,"value":checkpoint,"phase":"running",
        "previous":null,"runtime":fixture.run,"handoff":handoff});
    let request = fixture.request("checkpoint", input.clone());
    let before = fixture.files();
    let output = invoke(&request);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(before, fixture.files());
    let mut changed = input.clone();
    changed["attempt"]["started_at"] = json!("2026-10-24T00:00:00.000Z");
    reject(&invoke(&fixture.request("checkpoint", changed)));
    fixture.write(
        "controller-0001.json",
        &json!({"password":"secret-credential"}),
    );
    reject(&invoke(&fixture.request("checkpoint", input)));
}
