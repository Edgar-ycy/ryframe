//! 生命周期私有进程协议：检验输入拒绝和本地账本收口，不访问共享服务。
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const LIFECYCLE_ENTRY: &str = "__fixture-service-lifecycle";
const PRIMITIVE_ENTRY: &str = "__fixture-service-primitive";

fn backend() -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    #[cfg(windows)]
    let manifest = manifest.replace('/', "\\");
    Path::new(&manifest).parent().unwrap().to_path_buf()
}

fn invoke(entry: &str, payload: &[u8], extra: &[&str]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .current_dir(backend())
        .arg(entry)
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(payload).unwrap();
    child.wait_with_output().unwrap()
}

fn request(operation: &str, write: bool, owner: Value) -> Value {
    let root = backend();
    json!({"format_version":1,"backend":root,"operation":operation,
        "review":root.join(".local-tests/missing-review.json"),
        "environment":root.join(".local-tests/missing-environment.json"),"owner":owner,"write":write})
}

#[test]
fn explicit_write_and_owner_are_checked_before_opening_context() {
    for (operation, write, owner, expected) in [
        ("close", false, Value::Null, "--write"),
        ("status", true, Value::Null, "只读"),
        ("status", false, json!("credential-marker"), "owner"),
        ("restart", true, Value::Null, "owner"),
        ("close", true, json!("credential-marker"), "owner"),
        ("credential-marker", true, Value::Null, "未知"),
    ] {
        let output = invoke(
            LIFECYCLE_ENTRY,
            &serde_json::to_vec(&request(operation, write, owner)).unwrap(),
            &[],
        );
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stderr.is_empty());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["code"], "invalid_fixture_lifecycle");
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains(expected)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("credential-marker"));
    }
}

#[test]
fn duplicate_unknown_oversized_or_argv_payloads_cannot_execute() {
    let mut unknown = request("close", true, Value::Null);
    unknown["password"] = json!("credential-marker");
    for (payload, extra) in [
        (serde_json::to_vec(&unknown).unwrap(), vec![]),
        (
            br#"{"format_version":1,"format_version":1,"secret":"credential-marker"}"#.to_vec(),
            vec![],
        ),
        (vec![b'x'; 4 * 1024 * 1024 + 1], vec![]),
        (
            serde_json::to_vec(&request("close", true, Value::Null)).unwrap(),
            vec!["--write"],
        ),
    ] {
        let output = invoke(LIFECYCLE_ENTRY, &payload, &extra);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stderr.is_empty());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("credential-marker"));
    }
}

#[test]
fn escaped_context_is_rejected_without_creating_run_files() {
    let root = backend();
    let mut payload = request("close", true, Value::Null);
    payload["review"] = json!(root.join("Cargo.toml"));
    let output = invoke(LIFECYCLE_ENTRY, &serde_json::to_vec(&payload).unwrap(), &[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    assert!(!root.join("state.json").exists());
    assert!(!root.join("run.lock").exists());
}

#[test]
fn recovery_of_partial_startup_preserves_receipts_and_read_only_status() {
    let fixture = RecoveryFixture::new(1);
    let recovered = fixture.recover_and_check(false);
    assert_eq!(recovered["next_operation"], "status");

    let before = fixture.snapshot(None);
    let status = passed(fixture.lifecycle("status", false, Value::Null));
    assert_eq!(status["next_operation"], "reconcile-evidence");
    assert_eq!(status["state"], binding(&fixture.run.join("state.json")));
    assert_eq!(status["controller"], Value::Null);
    assert_eq!(status["remote_writes"], 0);
    assert_eq!(fixture.snapshot(None), before);
}

#[test]
fn recovery_of_interrupted_close_preserves_original_attempt_without_replay() {
    let fixture = RecoveryFixture::new(3);
    fixture.publish_interrupted_close();
    let recovered = fixture.recover_and_check(true);
    assert_eq!(recovered["next_operation"], "reconcile-evidence");
}

#[test]
fn recovery_of_completed_startup_rejects_a_held_guard_before_owner_recovery() {
    let fixture = RecoveryFixture::new(3);
    let guard = fixture.run.join("run-control.guard");
    let mut held = fs::File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&guard)
        .unwrap();
    held.write_all(&[0]).unwrap();
    held.try_lock().unwrap();

    // Windows 对已锁定的字节拒绝通过另一个句柄读取，其余全部文件仍逐字节核对。
    let before = fixture.snapshot(Some(&guard));
    let rejected = fixture.lifecycle("recover", true, json!(fixture.owner_binding));
    assert_eq!(rejected.status.code(), Some(2));
    assert!(rejected.stderr.is_empty());
    let rejected: Value = serde_json::from_slice(&rejected.stdout).unwrap();
    assert_eq!(rejected["error"]["code"], "invalid_fixture_lifecycle");
    assert_eq!(fixture.snapshot(Some(&guard)), before);
    assert_eq!(held.metadata().unwrap().len(), 1);
    drop(held);
    assert_eq!(fs::read(&guard).unwrap(), [0]);

    let recovered = fixture.recover_and_check(false);
    assert_eq!(recovered["next_operation"], "status");
}

struct RecoveryFixture {
    directory: tempfile::TempDir,
    review: PathBuf,
    bootstrap: PathBuf,
    run: PathBuf,
    owner_binding: PathBuf,
}

impl RecoveryFixture {
    fn new(initial_count: usize) -> Self {
        let local = backend().join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let directory = tempfile::tempdir_in(local).unwrap();
        let root = directory.path();
        let execution = root.join("execution");
        let future = execution
            .join(".local-tests")
            .join("reference-fixture")
            .join("run-r1");
        let run = future.join("service-run");
        for child in ["results", "rustfs", "redis", "run.lock"] {
            fs::create_dir_all(run.join(child)).unwrap();
        }
        let review = root.join("review.json");
        let bootstrap = root.join("bootstrap.json");
        let scope = "services-fixture-seed";
        write_json(
            &review,
            &json!({
                "kind":"review-only-perf-resource-plan-with-readonly-preflight",
                "future_root":future,"scopes":{"seed":{"backend_dir":execution}},
                "services":{"rustfs":{"scope_id":scope}}
            }),
        );
        write_json(
            &root.join("environment.json"),
            &json!({"environment":{"APP_ENV":"test"}}),
        );
        write_json(
            &bootstrap,
            &json!({
                "format_version":1,"kind":"reference-fixture-environment","status":"prepared",
                "services_started":false,"remote_writes":0,"plan":{"review":binding(&review)},
                "execution_backend":execution
            }),
        );
        let manifest = run.join("manifest.json");
        write_json(
            &manifest,
            &json!({
                "format_version":1,"kind":"reference-fixture-service-run","review":binding(&review),
                "bootstrap":binding(&bootstrap),"execution_backend":execution,"scope_id":scope,
                "data_directory_was_empty":true
            }),
        );
        let manifest = binding(&manifest);
        write_json(
            &run.join("state.json"),
            &json!({
                "format_version":1,"manifest":manifest,"attempts":initial_attempts(&run, initial_count)
            }),
        );
        // 保留原首代运行文件的形态，部分状态查询不得据此重放或接管服务。
        let tool = root.join("tool.exe");
        fs::write(&tool, b"tool").unwrap();
        let tool_binding = binding(&tool);
        write_json(
            &run.join("rustfs/process.json"),
            &json!({"role":"rustfs","lifecycle":"running",
                "identity":{"executable":tool,"sha256":tool_binding["sha256"]}}),
        );
        write_json(
            &run.join("rustfs/launch.json"),
            &json!({"status":"started"}),
        );
        write_json(
            &run.join("redis/runtime.json"),
            &json!({"redis":{"port":16390,"generation":"redis-r1"}}),
        );
        let owner = run.join("run.lock").join("owner.json");
        let owner_binding = root.join("owner-binding.json");
        write_json(
            &owner,
            &json!({
                "format_version":1,"directory":run,"manifest_sha256":manifest["sha256"],
                "identity":{"pid":2147483000_u32,"started":"1","executable":std::env::current_exe().unwrap()}
            }),
        );
        write_json(&owner_binding, &binding(&owner));
        Self {
            directory,
            review,
            bootstrap,
            run,
            owner_binding,
        }
    }

    fn publish_interrupted_close(&self) {
        let state_path = self.run.join("state.json");
        // 先前的 close 记录使用原生上下文绑定，模拟开始写入后控制器退出。
        let source = json!({"format_version":1,"backend":backend(),"operation":"service-context",
            "arguments":{"review":self.review,"environment":self.bootstrap}});
        let context = passed(invoke(
            "__fixture-service-history",
            &serde_json::to_vec(&source).unwrap(),
            &[],
        ));
        let mut state = read_json(&state_path);
        let attempts = state["attempts"].as_array_mut().unwrap();
        attempts.push(
            json!({"number":attempts.len() + 1,"stage":"fixture-services","mode":"close",
            "started_at":"2026-11-27T00:00:00Z","finished_at":null,"status":"running",
            "sources":context["sources"],"result":null,"error_type":null}),
        );
        write_json(&state_path, &state);
    }

    fn lifecycle(&self, operation: &str, write: bool, owner: Value) -> std::process::Output {
        let request = json!({"format_version":1,"backend":backend(),"operation":operation,
            "review":self.review,"environment":self.bootstrap,"owner":owner,"write":write});
        invoke(LIFECYCLE_ENTRY, &serde_json::to_vec(&request).unwrap(), &[])
    }

    fn snapshot(&self, exclude: Option<&Path>) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
        let mut entries = BTreeMap::new();
        collect_snapshot(self.directory.path(), exclude, &mut entries);
        entries
    }

    fn recover_and_check(&self, interrupted: bool) -> Value {
        let state = self.run.join("state.json");
        let state_before = binding(&state);
        let mut expected_state = read_json(&state);
        let owner = self.run.join("run.lock").join("owner.json");
        let saved_owner = read_json(&owner);
        let bound_owner = read_json(&self.owner_binding);
        assert_eq!(bound_owner, binding(&owner));
        let mut expected_files = self.snapshot(None);

        let result = passed(self.lifecycle("recover", true, json!(self.owner_binding)));
        assert_eq!(result["status"], "controller_recovered");
        assert_eq!(result["remote_writes"], 0);
        assert_eq!(result["resources_deleted"], false);
        assert!(!self.run.join("run.lock").exists());

        let mut state_after = read_json(&state);
        let recovery = state_after["attempts"]
            .as_array_mut()
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(recovery["status"], "passed");
        assert_eq!(recovery["stage"], "fixture-services");
        assert_eq!(recovery["mode"], "recover");
        if interrupted {
            let attempt = &mut expected_state["attempts"][3];
            let observed = &state_after["attempts"][3];
            assert!(observed["finished_at"].as_str().is_some());
            attempt["finished_at"] = observed["finished_at"].clone();
            attempt["status"] = json!("failed");
            attempt["error_type"] = json!("ControllerInterrupted");
        }
        assert_eq!(state_after, expected_state);

        let receipt = bound_path(&result["recovery"]);
        let id = receipt
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .strip_prefix("recovered-")
            .unwrap()
            .strip_suffix(".json")
            .unwrap();
        assert_eq!(receipt.parent(), Some(self.run.as_path()));
        let intent = self.run.join(format!("recovery-{id}.intent.json"));
        let state_after_sha =
            hex::encode(Sha256::digest(serde_json::to_vec(&state_after).unwrap()));
        assert_eq!(
            read_json(&receipt),
            json!({"status":"controller_recovered","owner":saved_owner,
            "remote_writes":0,"copy_requires_reconciliation":true,"state_sha256":state_after_sha})
        );
        assert_eq!(
            read_json(&intent),
            json!({"status":"controller_recovery_started","owner":bound_owner,
            "state_before":state_before,"state_after_sha256":state_after_sha})
        );
        let recovery_result = bound_path(&recovery["result"]);
        assert_eq!(read_json(&recovery_result), result);

        expected_files.remove(&self.run.join("run.lock"));
        expected_files.remove(&owner);
        for path in [
            state,
            receipt,
            intent,
            recovery_result,
            bound_path(&result["controller"]),
        ] {
            expected_files.insert(path.clone(), Some(fs::read(path).unwrap()));
        }
        expected_files.insert(self.run.join("run-control.guard"), Some(vec![0]));
        assert_eq!(self.snapshot(None), expected_files);
        result
    }
}

fn initial_attempts(run: &Path, count: usize) -> Vec<Value> {
    [
        ("storage-target", "initial"),
        ("cache-target", "initial"),
        ("fixture-buckets", "prepare"),
    ]
    .into_iter()
    .take(count)
    .enumerate()
    .map(|(index, (stage, mode))| {
        let number = index + 1;
        let path = run.join("results").join(format!("{number:04}.json"));
        write_json(&path, &json!({"status":stage}));
        json!({"number":number,"stage":stage,"mode":mode,"started_at":"2026-11-27T00:00:00Z",
                "finished_at":"2026-11-27T00:00:00Z","status":"passed","sources":{"fixture":true},
                "result":binding(&path),"error_type":null})
    })
    .collect()
}

fn collect_snapshot(
    directory: &Path,
    exclude: Option<&Path>,
    entries: &mut BTreeMap<PathBuf, Option<Vec<u8>>>,
) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if Some(path.as_path()) == exclude {
            continue;
        }
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            collect_snapshot(&path, exclude, entries);
            entries.insert(path, None);
        } else {
            assert!(kind.is_file());
            entries.insert(path.clone(), Some(fs::read(path).unwrap()));
        }
    }
}

fn write_json(path: &Path, value: &Value) {
    let mut bytes = serde_json::to_vec_pretty(value).unwrap();
    bytes.push(b'\n');
    fs::write(path, bytes).unwrap();
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn binding(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap();
    let path = path.to_str().unwrap();
    #[cfg(windows)]
    let path = path.replace('/', "\\");
    json!({"path":path,"bytes":bytes.len(),"sha256":hex::encode(Sha256::digest(bytes))})
}

fn bound_path(descriptor: &Value) -> PathBuf {
    let path = PathBuf::from(descriptor["path"].as_str().unwrap());
    assert_eq!(binding(&path), *descriptor);
    path
}

fn passed(output: std::process::Output) -> Value {
    let mut response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(0), "{response}");
    assert!(output.stderr.is_empty());
    assert_eq!(response["ok"], true);
    assert_eq!(response.as_object().unwrap().len(), 2);
    response["result"].take()
}

#[test]
fn native_primitive_rejects_malformed_input_without_leaking_values() {
    let envelope = json!({
        "format_version": 1,
        "kind": "ryframe-fixture-service-primitive",
        "backend": "unused-backend",
        "native_tool": null,
        "operation": "storage-start",
        "arguments": {}
    });
    let mut unknown = envelope.clone();
    unknown["password"] = json!("private-marker");
    let duplicate = format!(
        r#"{{"format_version":"private-marker",{}"#,
        &envelope.to_string()[1..]
    );
    for (payload, extra) in [
        (serde_json::to_vec(&unknown).unwrap(), vec![]),
        (duplicate.into_bytes(), vec![]),
        (vec![b'x'; 4 * 1024 * 1024 + 1], vec![]),
        (
            serde_json::to_vec(&envelope).unwrap(),
            vec!["private-marker"],
        ),
    ] {
        let output = invoke(PRIMITIVE_ENTRY, &payload, &extra);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stderr.is_empty());
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(matches!(
            response["error"].as_str(),
            Some("ValueError" | "PrimitiveUnknown")
        ));
        assert_eq!(
            response,
            json!({
                "ok": false,
                "error": response["error"],
                "diagnostic": null,
                "exit_code": null,
                "cleanup_diagnostic": null
            })
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private-marker"));
    }
}
