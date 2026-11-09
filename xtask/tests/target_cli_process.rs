//! fresh-target CLI 及 prepare 状态的真实有界协议测试；不连接共享服务。
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

struct Fixture {
    directory: tempfile::TempDir,
    backend: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let backend = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let parent = backend.join(".local-tests/target-cli-process");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("中文 空格-")
            .tempdir_in(parent)
            .unwrap();
        Self { directory, backend }
    }

    fn path(&self, name: &str) -> PathBuf {
        name.split('/')
            .fold(self.directory.path().to_path_buf(), |path, part| {
                path.join(part)
            })
    }

    fn write(&self, name: &str, value: &Value) -> Value {
        let path = self.path(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        descriptor(&path)
    }

    fn invoke(&self, target: &Path, operation: &str, arguments: Value) -> Value {
        let request = json!({"format_version":1,"operation":operation,"backend":self.backend,"target":target,"arguments":arguments});
        let before = files(self.directory.path());
        let response = raw(
            &["__target-evidence"],
            &serde_json::to_vec(&request).unwrap(),
        );
        assert_eq!(
            files(self.directory.path()),
            before,
            "只读状态与计划不能写入现场"
        );
        response
    }

    fn state(&self, target: &Path, request: &Value) -> Value {
        self.invoke(
            target,
            "prepare-resume-state",
            json!({"request_descriptor":request}),
        )
    }

    fn prepare(&self, request: &Value) -> Value {
        let result = self.invoke(
            &self.path("target"),
            "prepare-document",
            json!({"at":"2026-10-23T00:00:00Z",
            "request_descriptor":request,"generation":{"source":"fixture"},"absence":{}}),
        );
        assert_eq!(result["ok"], true, "{result}");
        self.write("target/prepare.json", &result["result"])
    }

    fn failure(&self) {
        self.write("target/failure.json",&json!({"status":"needs_reconciliation","stage":"prepare_binding",
            "at":"2026-10-23T00:00:00Z","error_type":"ValueError","reason":"private-secret",
            "automatic_retry":false,"automatic_resource_cleanup":false,"fresh_target_initialized":false,
            "clone_verified":false,"restore_qualified":false}));
    }

    fn resume(&self, request: &Value) -> (Value, Value) {
        let attempt = "a".repeat(32);
        let mut before = serde_json::Map::new();
        for name in ["request.json", "prepare.json", "failure.json"] {
            before.insert(
                name.to_owned(),
                descriptor(&self.path(&format!("target/{name}"))),
            );
        }
        let intent = self.write(
            &format!("target/resume-prepare-{attempt}.intent.json"),
            &json!({
            "format_version":1,"kind":"devex-clone-prepare-resume-intent","attempt":attempt,
            "at":"2026-10-23T00:00:01Z","request":request,"files_before":before}),
        );
        let terminal = json!({"format_version":1,"kind":"devex-clone-prepare-resume-confirmed","attempt":attempt,
            "at":"2026-10-23T00:00:02Z","intent":intent,"prepare":descriptor(&self.path("target/prepare.json")),
            "request":descriptor(&self.path("target/request.json")),"remote_writes":0});
        self.write(
            &format!("target/resume-prepare-{attempt}.confirmed.json"),
            &terminal,
        );
        (intent, terminal)
    }
}

fn descriptor(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap();
    json!({"path":path,"bytes":bytes.len(),"sha256":hex::encode(Sha256::digest(bytes))})
}

fn files(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut result = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(files(&path));
        } else {
            result.push((path.clone(), fs::read(path).unwrap()));
        }
    }
    result.sort_by(|left, right| left.0.cmp(&right.0));
    result
}

fn raw(args: &[&str], bytes: &[u8]) -> Value {
    let mut process = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let write = process.stdin.take().unwrap().write_all(bytes);
    let output = process.wait_with_output().unwrap();
    assert!(write.is_ok() || !output.status.success());
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        output.status.code(),
        Some(if value["ok"] == true { 0 } else { 1 })
    );
    value
}

fn rejected(value: &Value) {
    assert_eq!(value["ok"], false, "{value}");
    assert_eq!(value["error"]["category"], "TargetEvidenceRejected");
    assert_eq!(value["error"]["remote_writes"], 0);
    assert_eq!(value["error"]["automatic_retry"], false);
    assert!(!value.to_string().contains("private-secret"));
}

#[test]
fn missing_target_is_resumable_and_unknown_writes_never_become_replayable() {
    let f = Fixture::new();
    let request = f.write("request.json", &json!({"target":{"databases":[]}}));
    let target = f.path("target");
    assert_eq!(
        f.state(&target, &request)["result"],
        json!({"resumable":true,"prepared":false,"reason":null,"files":{},"pending_attempts":[]})
    );
    fs::create_dir(&target).unwrap();
    for name in [
        "create-shared-control.intent.json",
        "sentinel.intent.json",
        "reset.intent.json",
    ] {
        f.write(&format!("target/{name}"), &json!({}));
        let state = f.state(&target, &request);
        assert_eq!(state["result"]["resumable"], false);
        assert_eq!(state["result"]["reason"], "已存在资源写入 intent");
        fs::remove_file(target.join(name)).unwrap();
    }
    fs::write(target.join("unknown.txt"), b"private-secret").unwrap();
    assert_eq!(f.state(&target, &request)["result"]["resumable"], false);
}

#[test]
fn prepare_requires_unchanged_bound_request_and_rejects_forged_published_request() {
    let f = Fixture::new();
    let request = f.write("request.json", &json!({"target":{"databases":[]}}));
    f.write("target/request.json", &json!({"target":{"databases":[]}}));
    f.prepare(&request);
    assert_eq!(
        f.state(&f.path("target"), &request)["result"]["prepared"],
        true
    );
    let observed: Value =
        serde_json::from_slice(&fs::read(f.path("target/prepare.json")).unwrap()).unwrap();
    let mut current = json!({"at":observed["prepared_at"],"request_descriptor":request,
        "generation":observed["generation"],"absence":observed["absence"],"observed":observed});
    assert_eq!(
        f.invoke(&f.path("target"), "prepare-current", current.clone())["ok"],
        true
    );
    current["generation"]["source"] = json!("private-secret");
    rejected(&f.invoke(&f.path("target"), "prepare-current", current));
    f.write("target/request.json", &json!({"private-secret":true}));
    rejected(&f.state(&f.path("target"), &request));
    f.write("request.json", &json!({"private-secret":true}));
    rejected(&f.state(&f.path("target"), &request));
}

#[test]
fn prepare_resume_binds_exact_causal_chain_and_does_not_copy_failure_secret() {
    let f = Fixture::new();
    let request = f.write("request.json", &json!({"target":{"databases":[]}}));
    f.write("target/request.json", &json!({"target":{"databases":[]}}));
    f.prepare(&request);
    f.failure();
    rejected(&f.state(&f.path("target"), &request));
    let (intent, mut terminal) = f.resume(&request);
    let result = f.state(&f.path("target"), &request);
    assert_eq!(result["ok"], true, "{result}");
    assert!(!result.to_string().contains("private-secret"));
    assert_eq!(
        f.invoke(&f.path("target"), "prepare-failure", json!({}))["result"],
        json!({"present":true})
    );
    let mut bad: Value =
        serde_json::from_slice(&fs::read(intent["path"].as_str().unwrap()).unwrap()).unwrap();
    bad["at"] = json!("invalid-time-private-secret");
    fs::write(
        intent["path"].as_str().unwrap(),
        serde_json::to_vec(&bad).unwrap(),
    )
    .unwrap();
    let invalid = f.state(&f.path("target"), &request);
    rejected(&invalid);
    assert_eq!(
        invalid["error"]["message"],
        "fresh 目标 prepare 续作 intent 无效"
    );
    bad["at"] = json!("2026-10-23T00:00:03Z");
    fs::write(
        intent["path"].as_str().unwrap(),
        serde_json::to_vec(&bad).unwrap(),
    )
    .unwrap();
    terminal["intent"] = descriptor(Path::new(intent["path"].as_str().unwrap()));
    f.write(
        &format!("target/resume-prepare-{}.confirmed.json", "a".repeat(32)),
        &terminal,
    );
    let reversed = f.state(&f.path("target"), &request);
    rejected(&reversed);
    assert_eq!(
        reversed["error"]["message"],
        "fresh 目标 prepare 续作结果不属于原 intent"
    );
}

#[test]
fn registration_binds_request_environment_and_storage_without_leaking_values() {
    let f = Fixture::new();
    let request = f.write("request.json", &json!({"password":"private-secret"}));
    let environment = f.write(
        "environment.json",
        &json!({"environment":{"APP_ENV":"test"}}),
    );
    let manifest = f.write("storage/manifest.json", &json!({"kind":"fixture"}));
    let state = f.write("storage/state.json", &json!({}));
    let mut registered = json!({"format_version":1,"kind":"devex-clone-fresh-target-registration","request":request,
        "environment":environment,"storage_run":{"path":f.path("storage"),"manifest":manifest,"state":state},
        "target_directory":f.path("workspace/target")});
    f.write("workspace/registration.json", &registered);
    let result = f.invoke(&f.path("workspace"), "cli-registration", json!({}));
    assert_eq!(result["ok"], true, "{result}");
    assert!(!result.to_string().contains("private-secret"));
    registered["target_directory"] = json!(f.path("outside"));
    f.write("workspace/registration.json", &registered);
    rejected(&f.invoke(&f.path("workspace"), "cli-registration", json!({})));
    registered["target_directory"] = json!(f.path("workspace/target"));
    registered["password"] = json!("private-secret");
    f.write("workspace/registration.json", &registered);
    rejected(&f.invoke(&f.path("workspace"), "cli-registration", json!({})));
}

#[test]
fn stdin_protocol_rejects_argv_duplicate_fields_versions_and_oversized_requests() {
    for (args, bytes) in [
        (vec!["__target-evidence", "private-secret"], b"{}".to_vec()),
        (
            vec!["__target-evidence"],
            b"{\"operation\":\"private-secret\",\"operation\":\"cli-status\"}".to_vec(),
        ),
        (vec!["__target-evidence"], vec![b'x'; 1024 * 1024 + 1]),
    ] {
        rejected(&raw(&args, &bytes));
    }
    let f = Fixture::new();
    let mut request = json!({"format_version":2,"operation":"cli-dispatch","backend":f.backend,"target":f.path("missing"),
        "arguments":{"operation":"initialize","write":true,"supplied":[false,false,false,false]}});
    rejected(&raw(
        &["__target-evidence"],
        &serde_json::to_vec(&request).unwrap(),
    ));
    request["format_version"] = json!(1);
    request["arguments"]["private-secret"] = json!("private-secret");
    rejected(&raw(
        &["__target-evidence"],
        &serde_json::to_vec(&request).unwrap(),
    ));
}

#[test]
fn chinese_space_and_long_paths_are_supported_but_overlapping_directories_are_rejected() {
    let f = Fixture::new();
    let mut long = f.path("中文 空格");
    while long.to_string_lossy().chars().count() <= 280 {
        long.push("长目录".repeat(8));
    }
    fs::create_dir_all(&long).unwrap();
    let response = f.invoke(
        &long,
        "cli-dispatch",
        json!({"operation":"status","write":false,"supplied":[false,false,false,false]}),
    );
    assert_eq!(response["ok"], true, "{response}");
    let sibling = f.path("other");
    assert_eq!(
        f.invoke(&long, "cli-disjoint", json!({"paths":[long,sibling]}))["ok"],
        true
    );
    rejected(&f.invoke(
        &long,
        "cli-disjoint",
        json!({"paths":[long,long.join("nested")]}),
    ));
}
