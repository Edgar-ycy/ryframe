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
fn formal_rebind_rejects_invalid_manifest_without_python_or_new_result() {
    let fixture = Fixture::new();
    fixture.write("manifest.json", &json!({}));
    fixture.write("successor.json", &json!({}));
    let before = fixture.files();
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args([
            "check",
            "recovery",
            "clone",
            "seed-runtime",
            "--operation",
            "source-rebind",
            "--run-dir",
        ])
        .arg(&fixture.run)
        .arg("--request")
        .arg(fixture.run.join("successor.json"))
        .arg("--write")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Traceback"));
    assert_eq!(fixture.files(), before);
    assert!(!fixture.run.join("run.lock").exists());
    assert!(!fixture.run.join("results").exists());
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
fn rebind_history_plan_uses_bound_read_only_protocol_and_rejects_malformed_inputs() {
    let fixture = Fixture::new();
    let published = json!({"number":52,"stage":"seed-runtime","mode":"source-register",
        "status":"passed","result":{"published":true}});
    let active = json!({"number":53,"stage":"seed-runtime","mode":"source-rebind","status":"running","result":null});
    let args = json!({"attempts":[published,active],"descriptor":{"published":true},"current":53,
        "archive":null,"segment":null,"pending":[],"backend_available":true,
        "cache_recovery":false,"generation_output_exists":false});
    let request = fixture.request("rebind-model", json!({"action":"history","input":args}));
    let before = fixture.files();
    let output = invoke(&request);
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        response["result"]["value"]["actions"],
        json!([{"kind":"owner"}])
    );
    assert_eq!(response["result"]["remote_writes"], 0);
    assert_eq!(response["result"]["automatic_retry_permitted"], false);
    assert_eq!(before, fixture.files());
    for field in [
        "backend_available",
        "cache_recovery",
        "generation_output_exists",
    ] {
        let mut bad = args.clone();
        bad[field] = json!("secret-credential");
        reject(&invoke(&fixture.request(
            "rebind-model",
            json!({"action":"history","input":bad}),
        )));
    }
    let mut bad = args;
    bad["attempts"][1]["number"] = json!(true);
    reject(&invoke(&fixture.request(
        "rebind-model",
        json!({"action":"history","input":bad}),
    )));
}

#[test]
fn rebind_receipt_never_publishes_after_changed_second_observation() {
    let fixture = Fixture::new();
    let output = invoke(&fixture.request("rebind-model",json!({"action":"register-receipt",
        "input":{"before":{"source":"original"},"after":{"source":"secret-credential"},"request":{}}})));
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        response["result"]["value"]["rejected"]
            .as_str()
            .unwrap()
            .contains("历史变化")
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("secret-credential"));
    assert!(!fixture.run.join("results").exists());
    let request = fixture.request(
        "rebind-model",
        json!({"action":"secret-credential","input":{}}),
    );
    reject(&invoke(&request));
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

#[test]
fn lineage_generation_models_use_existing_bound_protocol_without_writes() {
    let fixture = Fixture::new();
    let parameters = json!({"action":"lineage","input":{"action":"pending","input":{
        "attempts":[{"number":68,"stage":"seed-runtime","mode":"source-generation-start",
            "status":"failed","result":null,"error_type":"ValueError"}],"previous":67}}});
    let mut request: Value =
        serde_json::from_slice(&fixture.request("coordinator", parameters)).unwrap();
    request["operation"] = json!("generation-guard");
    let before = fixture.files();
    let output = invoke(&serde_json::to_vec(&request).unwrap());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        response["result"],
        json!({"verified":true,"value":{"pending":true},
        "remote_writes":0,"automatic_retry_permitted":false})
    );
    assert_eq!(before, fixture.files());
    fixture.write("parameters.json", &json!({"changed":true}));
    reject(&invoke(&serde_json::to_vec(&request).unwrap()));
}

#[test]
fn lineage_model_rejections_redact_inputs_and_never_authorize_retry() {
    let fixture = Fixture::new();
    let input = json!({"action":"lineage","input":{"action":"request","input":{
        "previous":{},"request":{"format_version":1,"kind":"secret-credential"},"external":true}}});
    let mut request: Value =
        serde_json::from_slice(&fixture.request("coordinator", input)).unwrap();
    request["operation"] = json!("generation-guard");
    let before = fixture.files();
    let output = invoke(&serde_json::to_vec(&request).unwrap());
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        response["result"]["value"]["rejected"],
        "C68 谱系恢复请求与失败代次不同"
    );
    assert_eq!(response["result"]["remote_writes"], 0);
    assert_eq!(response["result"]["automatic_retry_permitted"], false);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("secret-credential"));
    assert!(output.stderr.is_empty());
    assert_eq!(before, fixture.files());
}
