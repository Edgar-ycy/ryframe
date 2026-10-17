use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    cli::CloneCommand,
    recovery::clone::native::export::{
        input::Context,
        object, recovery,
        transport::{CommandOutput, CommandSpec, Transport},
    },
    recovery::clone::native::{
        self,
        lifecycle::{
            stage::{self, StageRequest, StageRunner},
            state,
        },
    },
    workspace,
};

static NEXT: AtomicUsize = AtomicUsize::new(1);
const UUID: &str = "67cd8d4a-fe90-11ef-98fd-5847ca786499";

struct Fixture {
    backend: PathBuf,
    local: PathBuf,
    run: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let backend = workspace::root_dir();
        let local = backend.join(".local-tests").join(format!(
            "export-native-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&local).unwrap();
        let (runtime, defaults) = write_support_files(&backend, &local);
        let copy = local.join("copy");
        fs::create_dir(&copy).unwrap();
        let request = local.join("source-request.json");
        write_request(&local, &runtime, &defaults, &request);
        write_manifest(&local, &copy, &request);
        let run = local.join("run");
        native::run(
            &CloneCommand::Init {
                manifest: local.join("manifest.json"),
                run_dir: run.clone(),
            },
            &backend,
        )
        .unwrap();
        Self {
            backend,
            local,
            run,
        }
    }
}

fn write_support_files(backend: &Path, local: &Path) -> (PathBuf, PathBuf) {
    let runtime = local.join("runtime");
    fs::create_dir(&runtime).unwrap();
    write_json(
        &runtime.join("runtime.json"),
        &json!({"backend_root": backend, "scope_id": "fixture-source"}),
    );
    for role in ["api", "worker"] {
        write_json(
            &runtime.join(format!("{role}.json")),
            &json!({
                "format_version": 1,
                "role": role,
                "scope_id": "fixture-source",
                "identity": {
                    "pid": 100,
                    "started": "12345",
                    "executable": local.join(format!("{role}.exe")),
                },
            }),
        );
    }
    let defaults = local.join("client.cnf");
    fs::write(
        &defaults,
        "[client]\nhost=127.0.0.1\nport=3306\nuser=fixture\npassword=fixture-secret\nssl-mode=DISABLED\n",
    )
    .unwrap();
    for name in ["mysql", "mysqldump", "aws", "node"] {
        fs::write(local.join(name), name.as_bytes()).unwrap();
    }
    for name in ["backend-build.json", "maintenance.json", "producers.json"] {
        write_json(&local.join(name), &json!({"kind": name}));
    }
    for name in ["source-environment.json", "target-environment.json"] {
        write_json(&local.join(name), &json!({"environment": {}}));
    }
    write_json(
        &local.join("initialized.json"),
        &json!({"kind": "fixture-target"}),
    );
    (runtime, defaults)
}

fn write_request(local: &Path, runtime: &Path, defaults: &Path, request: &Path) {
    write_json(
        request,
        &json!({
            "format_version": 1,
            "kind": "devex-clone-source-export",
            "id": "fixture-export",
            "source": {
                "scope_id": "fixture-source",
                "runtime_dir": runtime,
                "api_url": "http://127.0.0.1:18210",
                "s3": {
                    "endpoint": "http://127.0.0.1:29200",
                    "region": "us-east-1",
                    "access_key_env": "TEST_ACCESS",
                    "secret_key_env": "TEST_SECRET",
                },
                "databases": [{
                    "key": "shared-control",
                    "kind": "combined",
                    "mode": "shared",
                    "database": "fixture_shared_control",
                    "server_uuid": UUID,
                    "defaults_file": defaults,
                    "defaults_sha256": sha(&fs::read(defaults).unwrap()),
                }],
            },
            "tools": {
                "mysql": tool(local, "mysql"),
                "mysqldump": tool(local, "mysqldump"),
                "aws": tool(local, "aws"),
                "node": tool(local, "node"),
            },
            "backend_build": binding(&local.join("backend-build.json")),
            "maintenance_build": binding(&local.join("maintenance.json")),
            "worktree_fingerprint": format!("sha256:{}", "a".repeat(64)),
            "runtime": binding(&runtime.join("runtime.json")),
            "processes": {
                "api": binding(&runtime.join("api.json")),
                "worker": binding(&runtime.join("worker.json")),
            },
            "producers_registry": binding(&local.join("producers.json")),
            "max_object_bytes": 1024,
        }),
    );
}

fn write_manifest(local: &Path, copy: &Path, request: &Path) {
    write_json(
        &local.join("manifest.json"),
        &json!({
            "format_version": 1,
            "kind": "devex-clone-run",
            "id": "fixture-export",
            "source_request": binding(request),
            "source_export": Value::Null,
            "initialized": binding(&local.join("initialized.json")),
            "source_environment": binding(&local.join("source-environment.json")),
            "target_environment": binding(&local.join("target-environment.json")),
            "copy_directory": copy,
            "copy_stage": "source_to_seed",
            "build_bridges": [],
        }),
    );
}

impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(
            self.local
                .starts_with(workspace::root_dir().join(".local-tests"))
        );
        let _ = fs::remove_dir_all(&self.local);
    }
}

#[test]
fn export_preflight_is_native_and_read_only() {
    let fixture = Fixture::new();
    let before = snapshot(&fixture.run);
    let request = StageRequest {
        backend: &fixture.backend,
        run_dir: &fixture.run,
        stage: "export",
        mode: "run",
        attempt: None,
    };
    StageRunner::preflight(&stage::ExportRunner::new(&fixture.backend), &request).unwrap();
    assert!(!fixture.run.join("run.lock").exists());
    assert!(!fixture.run.join("export-0001.intent.json").exists());
    let state: Value =
        serde_json::from_slice(&fs::read(fixture.run.join("state.json")).unwrap()).unwrap();
    assert_eq!(state["attempts"], json!([]));
    assert_eq!(snapshot(&fixture.run), before);
}

#[test]
fn export_preflight_rejects_linked_tool_paths() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(windows)]
    use std::os::windows::fs::symlink_file;

    let fixture = Fixture::new();
    let request_path = fixture.local.join("source-request.json");
    let mut request: Value = serde_json::from_slice(&fs::read(&request_path).unwrap()).unwrap();
    let linked = fixture.local.join("linked-aws");
    #[cfg(unix)]
    let result = symlink(fixture.local.join("aws"), &linked);
    #[cfg(windows)]
    let result = symlink_file(fixture.local.join("aws"), &linked);
    if let Err(error) = result {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            return;
        }
        panic!("无法建立链接夹具：{error}");
    }
    request["tools"]["aws"] = binding(&linked);
    write_json(&request_path, &request);
    let manifest = fixture.local.join("manifest.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    value["source_request"] = binding(&request_path);
    write_json(&manifest, &value);
    let run = fixture.local.join("linked-run");
    native::run(
        &CloneCommand::Init {
            manifest,
            run_dir: run.clone(),
        },
        &fixture.backend,
    )
    .unwrap();
    let request = StageRequest {
        backend: &fixture.backend,
        run_dir: &run,
        stage: "export",
        mode: "run",
        attempt: None,
    };
    let error = StageRunner::preflight(&stage::ExportRunner::new(&fixture.backend), &request)
        .unwrap_err()
        .to_string();
    assert!(error.contains("链接") || error.contains("工具"));
}

struct FakeAws {
    wrong_body: bool,
}

impl Transport for FakeAws {
    fn run(&self, spec: CommandSpec) -> Result<CommandOutput, Box<dyn std::error::Error>> {
        let status = Command::new(if cfg!(windows) { "cmd" } else { "sh" })
            .args(if cfg!(windows) {
                ["/C", "exit 0"]
            } else {
                ["-c", "true"]
            })
            .status()?;
        let operation = spec
            .args
            .iter()
            .position(|value| value == "s3api")
            .and_then(|index| spec.args.get(index + 1))
            .ok_or("fixture aws operation missing")?
            .clone();
        if operation == "get-object" {
            let path = spec
                .args
                .last()
                .ok_or("fixture get-object destination missing")?;
            let owner = spec
                .args
                .iter()
                .any(|value| value.ends_with("/.ryframe-owner"));
            let body = if owner {
                format!(
                    "ryframe-owner:v1:fixture-source:object-storage:{}",
                    spec.args
                        .iter()
                        .position(|value| value == "--bucket")
                        .and_then(|index| spec.args.get(index + 1))
                        .ok_or("fixture bucket missing")?
                )
            } else if self.wrong_body {
                "wrong".into()
            } else {
                "data".into()
            };
            fs::write(path, body)?;
        }
        let response = if operation == "head-object" {
            json!({
                "ContentType": "text/plain",
                "Metadata": {},
                "ETag": "\"fixture-etag\"",
                "ContentLength": if self.wrong_body { 5 } else { 4 },
                "LastModified": "2026-09-04T00:00:00+00:00",
                "AcceptRanges": "bytes",
                "StorageClass": "STANDARD",
                "MissingMeta": 0,
                "TagCount": 0,
            })
        } else {
            json!({
                "ContentType": "text/plain",
                "Metadata": {},
                "ETag": "\"fixture-etag\"",
                "ContentLength": 4,
                "LastModified": "2026-09-04T00:00:00+00:00",
            })
        };
        Ok(CommandOutput {
            status,
            stdout: serde_json::to_vec(&response)?,
            stderr: Vec::new(),
        })
    }
}

#[test]
fn native_object_capture_uses_read_only_evidence_and_rejects_body_drift() {
    let fixture = Fixture::new();
    let run_dir = fixture.run.clone();
    fs::create_dir(run_dir.join("e0001")).unwrap();
    let request: Value =
        serde_json::from_slice(&fs::read(fixture.local.join("source-request.json")).unwrap())
            .unwrap();
    let context = Context {
        backend: fixture.backend.clone(),
        run_dir,
        mode: "run".into(),
        attempt: 1,
        manifest: json!({}),
        request,
        request_path: fixture.local.join("source-request.json"),
        environment: std::collections::BTreeMap::from([
            ("TEST_ACCESS".to_owned(), "access".to_owned()),
            ("TEST_SECRET".to_owned(), "secret".to_owned()),
        ]),
        execution_backend: fixture.backend.clone(),
    };
    let inventory = json!({
        "objects": [
            {"bucket": "uploads", "entries": [{"key": "fixture-source/file.txt", "bytes": 4, "sha256": sha(b"data")}]},
            {"bucket": "avatar", "entries": []},
            {"bucket": "exports", "entries": []},
            {"bucket": "imports", "entries": []},
            {"bucket": "config-packages", "entries": []}
        ]
    });
    let (objects, index) =
        object::capture(&context, &FakeAws { wrong_body: false }, &inventory, 1024).unwrap();
    assert_eq!(
        objects
            .iter()
            .find(|bucket| bucket["bucket"] == "uploads")
            .unwrap()["entries"][0]["artifact"]["sha256"],
        json!(sha(b"data"))
    );
    assert_eq!(index[&("uploads".into(), "file.txt".into())]["bytes"], 4);
    assert!(
        fixture
            .run
            .join("e0001/objects")
            .read_dir()
            .unwrap()
            .any(|entry| entry.unwrap().path().join("capture.json").is_file())
    );
    let capture_dir = fixture
        .run
        .join("e0001/objects")
        .read_dir()
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.join("capture.json").is_file())
        .unwrap();
    assert_python_capture_compatible(&fixture, &capture_dir);

    let failed = Fixture::new();
    fs::create_dir(failed.run.join("e0001")).unwrap();
    let mut failed_request: Value =
        serde_json::from_slice(&fs::read(failed.local.join("source-request.json")).unwrap())
            .unwrap();
    failed_request["source"]["scope_id"] = json!("fixture-source");
    let failed_context = Context {
        backend: failed.backend.clone(),
        run_dir: failed.run.clone(),
        mode: "run".into(),
        attempt: 1,
        manifest: json!({}),
        request: failed_request,
        request_path: failed.local.join("source-request.json"),
        environment: std::collections::BTreeMap::from([
            ("TEST_ACCESS".to_owned(), "access".to_owned()),
            ("TEST_SECRET".to_owned(), "secret".to_owned()),
        ]),
        execution_backend: failed.backend.clone(),
    };
    assert!(
        object::capture(
            &failed_context,
            &FakeAws { wrong_body: true },
            &inventory,
            1024
        )
        .is_err()
    );
}

fn assert_python_capture_compatible(fixture: &Fixture, capture_dir: &Path) {
    let script = r#"
import json
import sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from devex_clone_capture import capture_result, owner_evidence, read_json, verify_capture
from restore_build import file_digest
from restore_reference_io import ExternalTools
request = json.loads(Path(sys.argv[3]).read_text(encoding="utf-8"))
capture = Path(sys.argv[2])
tools = ExternalTools({"source": request["source"], "tools": request["tools"]}, capture.parent.parent)
intent = read_json(capture / "intent.json")
actual = file_digest(capture / "object.bin")
before = read_json(capture / "head-before.json")
get = read_json(capture / "get.json")
after = read_json(capture / "head-after.json")
before_owners = [owner_evidence(capture, "owner-before", bucket, request["source"]["scope_id"]) for bucket in ("avatar", "config-packages", "exports", "imports", "uploads")]
after_owners = [owner_evidence(capture, "owner-after", bucket, request["source"]["scope_id"]) for bucket in ("avatar", "config-packages", "exports", "imports", "uploads")]
calculated = capture_result(intent, before, get, after, actual, before_owners, after_owners)
stored = read_json(capture / "capture.json")
for key in sorted(set(calculated) | set(stored)):
    if calculated.get(key) != stored.get(key):
        print("DIFF", key, repr(calculated.get(key)), repr(stored.get(key)))
result = verify_capture(
    tools, "source", "uploads", request["source"]["scope_id"] + "/file.txt",
    capture, expected={"bytes": 4, "sha256": "%s"}, max_bytes=1024,
)
print(result["status"])
"#;
    let script = script.replace("%s", &sha(b"data"));
    let output = Command::new("python")
        .args([
            "-B",
            "-c",
            &script,
            &fixture.backend.join("tools/python").to_string_lossy(),
            &capture_dir.to_string_lossy(),
            &fixture.local.join("source-request.json").to_string_lossy(),
        ])
        .output()
        .expect("Python compatibility check must start");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "capture_evidence_verified"
    );
}

#[test]
fn complete_export_candidate_requires_explicit_reconcile() {
    let fixture = Fixture::new();
    let (number, _) =
        state::begin(&fixture.run, "export", "run", &json!({"source": "fixture"})).unwrap();
    assert_eq!(number, 1);
    state::finish(&fixture.run, number, None, Some("StageError")).unwrap();
    let output = fixture.run.join("e0001");
    fs::create_dir(&output).unwrap();
    fs::write(output.join("export.json"), b"{}").unwrap();
    let error = recovery::reject_reexport(&fixture.backend, &fixture.run, 2)
        .unwrap_err()
        .to_string();
    assert!(error.contains("reconcile"));
}

fn tool(root: &Path, name: &str) -> Value {
    let path = root.join(name);
    json!({"path": path, "sha256": sha(&fs::read(path).unwrap())})
}

fn binding(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap();
    json!({
        "path": path.to_string_lossy(),
        "bytes": bytes.len(),
        "sha256": sha(&bytes),
    })
}

fn write_json(path: &Path, value: &Value) {
    let mut text = serde_json::to_string_pretty(value).unwrap();
    text.push('\n');
    fs::write(path, text).unwrap();
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut result = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                pending.push(path);
            } else {
                result.push((path.clone(), fs::read(path).unwrap()));
            }
        }
    }
    result.sort_by(|left, right| left.0.cmp(&right.0));
    result
}
