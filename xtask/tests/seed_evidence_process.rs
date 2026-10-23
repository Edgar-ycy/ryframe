//! seed 私有协议的真实进程、边界、原始绑定与脱敏检查，不连接共享服务。
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
        let run = backend
            .join(".local-tests")
            .join(format!("seed-protocol-中文 空格-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(run.join("results")).unwrap();
        let fixture = Self { run };
        fixture.write("manifest.json", &json!({"fixture":true}));
        fixture.write("seed-runtime.json",&json!({"format_version":1,"kind":"devex-clone-seed-runtime",
            "run_manifest":fixture.binding("manifest.json"),"post_copy":{},"post_verify":{},
            "identity_plan":{},"identity_state":"fixture","identity_tool":{},"api_process":{},"node":{}}));
        fixture.write("results/0001.json",&json!({"status":"seed_runtime_registered","registration":fixture.binding("seed-runtime.json")}));
        fixture.write("state.json",&json!({"format_version":1,"manifest":fixture.binding("manifest.json"),"attempts":[{
            "number":1,"stage":"seed-runtime","mode":"register","started_at":"2026-10-23T00:00:00.000Z",
            "finished_at":"2026-10-23T00:00:01.000Z","status":"passed","sources":{},
            "result":fixture.binding("results/0001.json"),"error_type":null}]}));
        fixture
    }
    fn write(&self, name: &str, value: &Value) {
        fs::write(self.run.join(name), serde_json::to_vec(value).unwrap()).unwrap();
    }
    fn binding(&self, name: &str) -> Value {
        let path = self.run.join(name).components().collect::<PathBuf>();
        let bytes = fs::read(&path).unwrap();
        json!({"path":path,"bytes":bytes.len(),"sha256":hex::encode(Sha256::digest(bytes))})
    }
    fn request(&self, operation: &str, args: Value) -> Vec<u8> {
        serde_json::to_vec(
            &json!({"format_version":1,"kind":"ryframe-seed-evidence","run_dir":self.run,
            "operation":operation,"arguments":args}),
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.run).unwrap();
    }
}
fn invoke(bytes: &[u8], args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__seed-evidence")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if args.is_empty() {
        child.stdin.take().unwrap().write_all(bytes).unwrap();
    } else {
        drop(child.stdin.take());
    }
    child.wait_with_output().unwrap()
}
fn rejected(output: &Output) {
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["automatic_retry_permitted"], false);
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("secret-credential"));
}

#[test]
fn real_process_reads_original_published_registration_in_unicode_path_without_writes() {
    let fixture = Fixture::new();
    let before = fs::read(fixture.run.join("state.json")).unwrap();
    let output = invoke(
        &fixture.request("registration", json!({"cleanup":false})),
        &[],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(value["result"]["kind"], "devex-clone-seed-runtime");
    assert_eq!(before, fs::read(fixture.run.join("state.json")).unwrap());
    assert!(!fixture.run.join("seed-runtime").exists());
    assert!(!fixture.run.join("run.lock").exists());
    fixture.write(
        "seed-runtime.json",
        &json!({"password":"secret-credential"}),
    );
    rejected(&invoke(
        &fixture.request("registration", json!({"cleanup":true})),
        &[],
    ));
}

#[test]
fn real_process_rejects_duplicate_oversize_extra_argv_and_unowned_registration_without_echo() {
    for bytes in [
        b"{\"password\":\"secret-credential\",\"password\":1}".to_vec(),
        vec![b' '; 65537],
        vec![],
    ] {
        rejected(&invoke(&bytes, &[]));
    }
    rejected(&invoke(&[], &["--password", "secret-credential"]));
    let fixture = Fixture::new();
    let before = fs::read(fixture.run.join("seed-runtime.json")).unwrap();
    rejected(&invoke(
        &fixture.request(
            "register",
            json!({"request":fixture.run.join("secret-credential.json"),
        "attempt":2,"controller":{},"target_lock_inode":1}),
        ),
        &[],
    ));
    assert_eq!(
        before,
        fs::read(fixture.run.join("seed-runtime.json")).unwrap()
    );
    assert!(!fixture.run.join("run.lock").exists());
}

#[test]
fn generation_guard_reads_bound_snapshot_and_never_publishes_or_replays() {
    let fixture = Fixture::new();
    let backend = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    fixture.write(
        "previous.json",
        &json!({"frozen":"same","backend_build":{},"maintenance_build":{}}),
    );
    fixture.write("parameters.json", &json!({"operation":"archived-request","arguments":{
        "previous":fixture.binding("previous.json"),"request":{"frozen":"same","backend_build":{},"maintenance_build":{}},
        "segment":null}}));
    let request = fixture.request(
        "generation-guard",
        json!({"backend_root":backend,
        "parameters":fixture.binding("parameters.json")}),
    );
    let before = fs::read(fixture.run.join("state.json")).unwrap();
    let output = invoke(&request, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        response["result"],
        json!({"verified":true,"remote_writes":0,"automatic_retry_permitted":false})
    );
    fixture.write("previous.json", &json!({"password":"secret-credential"}));
    rejected(&invoke(&request, &[]));
    assert_eq!(before, fs::read(fixture.run.join("state.json")).unwrap());
    assert!(!fixture.run.join("intent.json").exists());
    assert!(!fixture.run.join("run.lock").exists());
}

#[test]
fn cancelled_generation_protocol_without_eof_creates_no_evidence_or_success_receipt() {
    let fixture = Fixture::new();
    let before = fs::read(fixture.run.join("state.json")).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__seed-evidence")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(b"{\"format_version\":1,\"kind\":\"ryframe-seed-evidence\"")
        .unwrap();
    input.flush().unwrap();
    assert!(child.try_wait().unwrap().is_none());
    child.kill().unwrap();
    let result = child.wait_with_output().unwrap();
    drop(input);
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert_eq!(before, fs::read(fixture.run.join("state.json")).unwrap());
    assert!(!fixture.run.join("intent.json").exists());
    assert!(!fixture.run.join("results/0002.json").exists());
}
