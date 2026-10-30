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

#[path = "export_native/recovery.rs"]
mod recovery_tests;

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
    let directory = local.join("config");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("app.test.toml"), "[object_storage]\nbackend='rustfs'\nendpoint='http://127.0.0.1:29200'\nregion='us-east-1'\naccess_key='private-fixture-access'\nsecret_key='private-fixture-secret'\n").unwrap();
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
    for name in ["mysql", "mysqldump", "node"] {
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
                "s3": {"configuration": native::s3::app_configuration::configuration_value(
                    &workspace::root_dir(), &local.join("config"), "test").unwrap()},
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
    let linked = fixture.local.join("linked-mysqldump");
    #[cfg(unix)]
    let result = symlink(fixture.local.join("mysqldump"), &linked);
    #[cfg(windows)]
    let result = symlink_file(fixture.local.join("mysqldump"), &linked);
    if let Err(error) = result {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            return;
        }
        panic!("无法建立链接夹具：{error}");
    }
    request["tools"]["mysqldump"] = binding(&linked);
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

struct FakeS3 {
    wrong_body: bool,
}

impl Transport for FakeS3 {
    fn run(&self, _: CommandSpec) -> Result<CommandOutput, Box<dyn std::error::Error>> {
        panic!("对象采集不得启动外部命令")
    }
    fn s3(
        &self,
        request: crate::recovery::clone::native::s3::Request,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        use crate::recovery::clone::native::s3::Operation;
        let head = matches!(request.operation, Operation::Head { .. });
        if let Operation::Get {
            destination, key, ..
        } = request.operation
        {
            let body = if key.ends_with("/.ryframe-owner") {
                format!(
                    "ryframe-owner:v1:fixture-source:object-storage:{}",
                    request.bucket
                )
            } else if self.wrong_body {
                "evil".into()
            } else {
                "data".into()
            };
            fs::write(destination, body)?;
        }
        let response = if head {
            json!({
                "ContentType": "text/plain",
                "Metadata": {},
                "ETag": "\"fixture-etag\"",
                "ContentLength": 4,
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
        Ok(response)
    }
}

#[test]
fn native_object_capture_uses_read_only_evidence_and_rejects_body_drift() {
    let fixture = Fixture::new();
    let context = object_capture_context(&fixture);
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
        object::capture(&context, &FakeS3 { wrong_body: false }, &inventory, 1024).unwrap();
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
    let failed_context = object_capture_context(&failed);
    let error = object::capture(
        &failed_context,
        &FakeS3 { wrong_body: true },
        &inventory,
        1024,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("SHA"), "{error}");
}

fn object_capture_context(fixture: &Fixture) -> Context {
    let run_dir = fixture.run.clone();
    fs::create_dir(run_dir.join("e0001")).unwrap();
    let request: Value =
        serde_json::from_slice(&fs::read(fixture.local.join("source-request.json")).unwrap())
            .unwrap();
    Context {
        backend: fixture.backend.clone(),
        output_dir: run_dir.join("e0001"),
        run_dir,
        mode: "run".into(),
        attempt: 1,
        manifest: json!({}),
        object_identity:
            crate::recovery::clone::native::s3::app_configuration::public_configuration(
                &fixture.backend,
                &request["source"]["s3"]["configuration"],
            )
            .unwrap(),
        request,
        request_path: fixture.local.join("source-request.json"),
        environment: std::collections::BTreeMap::from([
            ("TEST_ACCESS".to_owned(), "access".to_owned()),
            ("TEST_SECRET".to_owned(), "secret".to_owned()),
        ]),
        execution_backend: fixture.backend.clone(),
    }
}

#[test]
fn source_cli_reads_bound_sidecar_config_records_actual_argv_and_rejects_drift_before_spawn() {
    struct Configured {
        directory: String,
        arguments: Vec<String>,
    }
    impl Transport for Configured {
        fn run(&self, command: CommandSpec) -> crate::Result<CommandOutput> {
            assert_eq!(command.args[..2], ["--config-dir", self.directory.as_str()]);
            assert_eq!(command.args[2..], self.arguments);
            #[cfg(unix)]
            use std::os::unix::process::ExitStatusExt;
            #[cfg(windows)]
            use std::os::windows::process::ExitStatusExt;
            Ok(CommandOutput {
                status: std::process::ExitStatus::from_raw(0),
                stdout: vec![],
                stderr: vec![],
            })
        }
        fn s3(&self, _: native::s3::Request) -> crate::Result<Value> {
            panic!("来源CLI配置用例不能访问对象服务")
        }
    }
    let fixture = Fixture::new();
    let context = object_capture_context(&fixture);
    let directory = context.source()["s3"]["configuration"]["directory"]
        .as_str()
        .unwrap();
    for (index, arguments) in [
        vec!["control", "verify"],
        vec!["tenant-data", "verify", "--target", "shared"],
        vec!["target-inventory", "--target", "shared"],
        vec!["backup-inventory"],
    ]
    .into_iter()
    .enumerate()
    {
        let arguments = arguments.into_iter().map(str::to_owned).collect::<Vec<_>>();
        let transport = Configured {
            directory: directory.to_owned(),
            arguments: arguments.clone(),
        };
        let label = format!("configuration-{index}");
        native::export::database::run_logged(
            &context,
            &transport,
            &label,
            Path::new("bound-tool"),
            &arguments,
            None,
        )
        .unwrap();
        let record: Value = serde_json::from_slice(
            &fs::read(context.output().join(format!("{label}.command.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(record["command"][0], "--config-dir");
        assert_eq!(record["command"][1], directory);
    }
    fs::write(
        Path::new(directory).join("app.test.toml"),
        "scope_id='drifted'\n",
    )
    .unwrap();
    let transport = Configured {
        directory: directory.to_owned(),
        arguments: vec![],
    };
    assert!(
        native::export::database::run_logged(
            &context,
            &transport,
            "configuration-drift",
            Path::new("bound-tool"),
            &[],
            None
        )
        .is_err()
    );
    assert!(
        !context
            .output()
            .join("configuration-drift.command.json")
            .exists()
    );
}

fn assert_python_capture_compatible(fixture: &Fixture, capture_dir: &Path) {
    let script = r#"
import json
import sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from rate_limit_tool import install_verified_binding
install_verified_binding(sys.argv[4])
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
    capture, expected={"bytes": 4, "sha256": "%s"}, max_bytes=1024, transport="rust-s3-sigv4",
)
print(result["status"])
"#;
    let script = script.replace("%s", &sha(b"data"));
    let native_tool = crate::check::python_suite::binding(Path::new(env!("CARGO_BIN_EXE_xtask")))
        .unwrap()
        .expect("兼容校验必须显式绑定当前原生工具");
    let output = Command::new("python")
        .args([
            "-B",
            "-c",
            &script,
            &fixture.backend.join("tools/python").to_string_lossy(),
            &capture_dir.to_string_lossy(),
            &fixture.local.join("source-request.json").to_string_lossy(),
            &native_tool,
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

#[test]
fn public_source_request_uses_bound_files_without_credentials_or_overrides() {
    use crate::recovery::clone::native::export::input::{public_request, validate_request};
    let fixture = Fixture::new();
    let path = fixture.local.join("source-request.json");
    let request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let original = request.clone();
    let public = public_request(&fixture.backend, &request).unwrap();
    for value in [&public, &request] {
        let encoded = value.to_string();
        assert!(!encoded.contains("private-fixture"));
        assert!(!encoded.contains("access_key"));
        assert!(!encoded.contains("secret_key"));
    }
    assert_eq!(
        public["source"]["s3"],
        json!({"endpoint":"http://127.0.0.1:29200","region":"us-east-1"})
    );
    assert_eq!(public["tools"], request["tools"]);
    assert_eq!(request, original);
    let mut override_request = request.clone();
    override_request["source"]["s3"]["endpoint"] = json!("http://127.0.0.1:9");
    assert!(validate_request(&fixture.backend, &override_request).is_err());
    override_request["source"]["s3"] = native::s3::app_configuration::private_configuration(
        &fixture.backend,
        &request["source"]["s3"]["configuration"],
    )
    .unwrap();
    assert!(validate_request(&fixture.backend, &override_request).is_err());
    fs::write(
        fixture.local.join("config/app.test.toml"),
        "[object_storage]\nsecret_key='changed-private-value'\n",
    )
    .unwrap();
    let error = public_request(&fixture.backend, &request)
        .unwrap_err()
        .to_string();
    assert!(!error.contains("changed-private-value"));
    assert!(validate_request(&fixture.backend, &request).is_err());
}

#[test]
fn source_registration_context_uses_bound_generation_without_export_stage_or_writes() {
    let fixture = Fixture::new();
    let request_path = fixture.local.join("source-request.json");
    let environment_path = fixture.local.join("registration-environment.json");
    write_json(
        &environment_path,
        &json!({"environment": {"APP_ENV": "test"}}),
    );
    let (attempt, _) =
        state::begin(&fixture.run, "seed-runtime", "source-register", &json!({})).unwrap();
    let before = snapshot(&fixture.run);
    let context = Context::from_source_registration(
        &fixture.backend,
        &fixture.run,
        attempt,
        &binding(&request_path),
        &binding(&environment_path),
    )
    .unwrap();
    assert_eq!(context.mode, "source-register");
    assert_eq!(context.attempt, attempt);
    assert_eq!(context.request_path, request_path);
    assert_eq!(context.execution_backend, fixture.backend);
    assert_eq!(context.environment.get("APP_ENV").unwrap(), "test");
    assert_eq!(snapshot(&fixture.run), before);
    assert!(!fixture.run.join("run.lock").exists());
    let request_binding = binding(&request_path);
    let environment_binding = binding(&environment_path);
    for number in [0, attempt + 1] {
        assert!(
            Context::from_source_registration(
                &fixture.backend,
                &fixture.run,
                number,
                &request_binding,
                &environment_binding,
            )
            .is_err()
        );
    }
    fs::write(&environment_path, b"{}").unwrap();
    assert!(
        Context::from_source_registration(
            &fixture.backend,
            &fixture.run,
            attempt,
            &request_binding,
            &environment_binding,
        )
        .is_err()
    );
}

#[test]
fn source_registration_context_rejects_wrong_stage_environment_and_tool_drift() {
    let fixture = Fixture::new();
    let request_path = fixture.local.join("source-request.json");
    let environment_path = fixture.local.join("registration-environment.json");
    let (attempt, _) = state::begin(&fixture.run, "export", "run", &json!({})).unwrap();
    write_json(
        &environment_path,
        &json!({"environment": {"APP_ENV": "test"}}),
    );
    assert!(
        Context::from_source_registration(
            &fixture.backend,
            &fixture.run,
            attempt,
            &binding(&request_path),
            &binding(&environment_path),
        )
        .is_err()
    );
    state::finish(&fixture.run, attempt, None, Some("FixtureStopped")).unwrap();
    let (attempt, _) =
        state::begin(&fixture.run, "seed-runtime", "source-register", &json!({})).unwrap();
    for environment in [
        json!({}),
        json!({"APP_ENV": "dev"}),
        json!({"APP_ENV": "test", "UNEXPECTED": "private-value"}),
    ] {
        write_json(&environment_path, &json!({"environment": environment}));
        assert!(
            Context::from_source_registration(
                &fixture.backend,
                &fixture.run,
                attempt,
                &binding(&request_path),
                &binding(&environment_path),
            )
            .is_err()
        );
    }
    write_json(
        &environment_path,
        &json!({"environment": {"APP_ENV": "test"}}),
    );
    let request_binding = binding(&request_path);
    fs::write(fixture.local.join("mysql"), b"changed-tool").unwrap();
    assert!(
        Context::from_source_registration(
            &fixture.backend,
            &fixture.run,
            attempt,
            &request_binding,
            &binding(&environment_path),
        )
        .is_err()
    );
}

// 此处只验证正式父入口拒绝未完成的 C69，不把最小导出夹具当作有效 START。
#[test]
fn native_generation_recovery_rejects_unqualified_c69_before_claiming_a_new_attempt() {
    use crate::cli::{SeedSourceOperation, SeedSourceOptions};
    use crate::recovery::source::evidence::control;

    let sources = native::source::current(&workspace::root_dir()).unwrap();
    for (error_type, published) in [
        (Some("StageError"), false),
        (Some("SourceChanged"), true),
        (None, false),
    ] {
        let fixture = Fixture::new();
        append_unqualified_c69(&fixture, &sources, error_type, published);
        let before = snapshot(&fixture.run);
        let options = SeedSourceOptions {
            operation: SeedSourceOperation::GenerationRecover,
            run_dir: fixture.run.clone(),
            request: Some(fixture.local.join("source-request.json")),
            write: true,
        };
        let error = control::run(&fixture.backend, &options).unwrap_err();
        let expected = if error_type.is_some() {
            "C69 谱系恢复失败或未发布，不能授权重试"
        } else {
            "源控制预检不能接管正在运行的阶段，保留控制器现场"
        };
        assert_eq!(error.to_string(), expected);
        assert_eq!(snapshot(&fixture.run), before);
        assert_eq!(
            state::attempts(&state::load(&fixture.run, true).unwrap())
                .unwrap()
                .len(),
            69
        );
        assert!(!fixture.run.join("run.lock").exists());
        assert!(!fixture.run.join("results/0070.json").exists());
    }
}

fn append_unqualified_c69(
    fixture: &Fixture,
    sources: &Value,
    error_type: Option<&str>,
    published: bool,
) {
    for expected in 1..=68 {
        let (number, _) = state::begin(&fixture.run, "post-copy", "run", sources).unwrap();
        assert_eq!(number, expected);
        state::finish(&fixture.run, number, None, Some("StageError")).unwrap();
    }
    let (number, _) = state::begin(
        &fixture.run,
        "seed-runtime",
        "source-generation-recover",
        sources,
    )
    .unwrap();
    assert_eq!(number, 69);
    let output = fixture.run.join("seed-runtime/attempt-0069");
    fs::create_dir_all(&output).unwrap();
    fs::write(
        output.join("partial-evidence.json"),
        b"{\"partial\":true}\n",
    )
    .unwrap();
    if let Some(error_type) = error_type {
        let receipt = json!({"status":"unqualified-local-result"});
        state::finish(
            &fixture.run,
            number,
            published.then_some(&receipt),
            Some(error_type),
        )
        .unwrap();
    }
}

#[test]
fn native_generation_status_preserves_missing_and_partial_start_without_repair() {
    use crate::cli::{SeedSourceOperation, SeedSourceOptions};
    use crate::recovery::source::evidence::control;

    let fixture = Fixture::new();
    let sources = native::source::current(&fixture.backend).unwrap();
    let (number, _) = state::begin(
        &fixture.run,
        "seed-runtime",
        "source-generation-start",
        &sources,
    )
    .unwrap();
    state::finish(&fixture.run, number, None, Some("StageError")).unwrap();
    let options = SeedSourceOptions {
        operation: SeedSourceOperation::GenerationStatus,
        run_dir: fixture.run.clone(),
        request: None,
        write: false,
    };
    let output = fixture.run.join(format!("g{number:04}"));
    for partial in [false, true] {
        if partial {
            fs::create_dir(&output).unwrap();
            write_json(&output.join("request.json"), &json!({"partial":true}));
        }
        let before = snapshot(&fixture.run);
        let observed = control::run(&fixture.backend, &options).unwrap();
        assert_eq!(observed["status"], "source_generation_observed");
        assert_eq!(observed["attempt"], number);
        assert_eq!(observed["start_status"], "failed");
        assert!(observed["intent"].is_null());
        for role in ["api", "worker"] {
            assert_eq!(
                observed["roles"][role],
                json!({"state":"unknown","tree":null,"completion":null})
            );
        }
        assert_eq!(observed["remote_writes"], 0);
        assert_eq!(observed["restore_qualified"], false);
        assert_eq!(snapshot(&fixture.run), before);
        assert!(!fixture.run.join("run.lock").exists());
    }
    let runtime = output.join("runtime");
    fs::create_dir(&runtime).unwrap();
    fs::write(runtime.join("binaries.json"), b"{}\n").unwrap();
    let before = snapshot(&fixture.run);
    assert_eq!(
        control::run(&fixture.backend, &options).unwrap()["roles"]["api"]["state"],
        "unknown"
    );
    assert_eq!(snapshot(&fixture.run), before);
    fs::write(runtime.join("api.json"), b"{}\n").unwrap();
    let before = snapshot(&fixture.run);
    assert!(control::run(&fixture.backend, &options).is_err());
    assert_eq!(snapshot(&fixture.run), before);
    fs::remove_file(runtime.join("api.json")).unwrap();
    fs::write(output.join("unbound.json"), b"{}").unwrap();
    let before = snapshot(&fixture.run);
    assert!(control::run(&fixture.backend, &options).is_err());
    assert_eq!(snapshot(&fixture.run), before);
    assert!(!fixture.run.join("run.lock").exists());
}

#[test]
fn native_generation_control_rejects_unauthorized_start_and_mutating_status_without_writes() {
    use crate::cli::{SeedSourceOperation, SeedSourceOptions};
    use crate::recovery::source::evidence::control;
    let fixture = Fixture::new();
    let request = fixture.local.join("source-request.json");
    for (operation, write, request) in [
        (
            SeedSourceOperation::GenerationStart,
            false,
            Some(request.clone()),
        ),
        (SeedSourceOperation::GenerationStart, true, None),
        (SeedSourceOperation::GenerationStatus, true, None),
        (SeedSourceOperation::GenerationStatus, false, Some(request)),
    ] {
        let before = snapshot(&fixture.run);
        let options = SeedSourceOptions {
            operation,
            run_dir: fixture.run.clone(),
            request,
            write,
        };
        assert!(control::run(&fixture.backend, &options).is_err());
        assert_eq!(snapshot(&fixture.run), before);
        assert!(!fixture.run.join("run.lock").exists());
    }
}
