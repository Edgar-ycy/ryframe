use std::{
    collections::BTreeMap,
    fs,
    net::{Ipv4Addr, TcpListener},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

use crate::{
    cli::CloneSide,
    recovery::clone::native::{
        lifecycle::state,
        resources::{
            common,
            storage::{self, model as storage_model},
        },
    },
    workspace,
};

static NEXT: AtomicUsize = AtomicUsize::new(1);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = workspace::root_dir().join(".local-tests").join(format!(
            "clone-resources-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn backend(&self) -> PathBuf {
        workspace::root_dir()
    }

    fn file(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, bytes).unwrap();
        path
    }

    fn json(&self, relative: &str, value: &Value) -> Value {
        let path = self.path(relative);
        common::write_new(&path, value).unwrap();
        common::binding(&path).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(
            self.root
                .starts_with(workspace::root_dir().join(".local-tests"))
        );
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(any(unix, windows))]
#[test]
fn target_storage_restart_binds_original_external_executable() {
    let outer = Fixture::new("target-storage-external");
    let executable = outer.file("tools/rustfs.exe", b"fixture-rustfs");
    let backend = outer.path("coordinator");
    let fixture = Fixture {
        root: backend.join(".local-tests/storage"),
    };
    let (run, manifest, request) = target_storage_fixture(&fixture, &backend, &executable);
    let check = |request: &Value| {
        storage_model::validate(&backend, &run, &manifest, CloneSide::Target, request)
    };
    assert!(!executable.starts_with(backend.join(".local-tests")));
    assert_eq!(check(&request).unwrap().private["APP_ENV"], "test");

    for (field, value) in [
        (
            "bytes",
            json!(request["executable"]["bytes"].as_u64().unwrap() + 1),
        ),
        ("sha256", json!("0".repeat(64))),
    ] {
        let mut changed = request.clone();
        changed["executable"][field] = value;
        assert_eq!(
            check(&changed).err().unwrap().to_string(),
            "RustFS 可执行文件与登记大小或摘要不一致",
            "{field}"
        );
    }

    let mut replacement = request.clone();
    replacement["executable"] = common::binding(&fixture.path("rustfs.exe")).unwrap();
    assert_eq!(
        replacement["executable"]["sha256"],
        request["executable"]["sha256"]
    );
    assert_eq!(
        check(&replacement).err().unwrap().to_string(),
        "存储重启不能替换原可执行文件"
    );

    let linked_directory = outer.path("linked-tools");
    #[cfg(unix)]
    create_directory_link(&outer.path("tools"), &linked_directory).unwrap();
    #[cfg(windows)]
    assert!(
        Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(&linked_directory)
            .arg(outer.path("tools"))
            .output()
            .unwrap()
            .status
            .success()
    );
    let mut linked = request;
    linked["executable"]["path"] = json!(linked_directory.join("rustfs.exe"));
    linked["previous"]["identity"]["executable"] = linked["executable"]["path"].clone();
    let error = check(&linked).err().unwrap().to_string();
    assert!(
        error.starts_with("RustFS 可执行文件无效") && error.contains("链接"),
        "{error}"
    );
    assert_eq!(fs::read(&executable).unwrap(), b"fixture-rustfs");
}

#[test]
fn storage_request_and_original_generation_are_exactly_bound() {
    let fixture = Fixture::new("storage-model");
    let (run_dir, manifest, request) = storage_fixture(&fixture);
    let validated = storage_model::validate(
        &fixture.backend(),
        &run_dir,
        &manifest,
        CloneSide::Source,
        &request,
    )
    .unwrap();
    assert_eq!(validated.private["PATH"], "fixture-bin");
    assert_eq!(
        storage_model::arguments(&request).unwrap(),
        vec![
            request["executable"]["path"].as_str().unwrap(),
            "server",
            "--address",
            request["api_url"]
                .as_str()
                .unwrap()
                .strip_prefix("http://")
                .unwrap(),
            "--console-address",
            request["console_url"]
                .as_str()
                .unwrap()
                .strip_prefix("http://")
                .unwrap(),
            "--console-enable",
            "--access-key-file",
            request["credential_files"]["access_key"]["path"]
                .as_str()
                .unwrap(),
            "--secret-key-file",
            request["credential_files"]["secret_key"]["path"]
                .as_str()
                .unwrap(),
            request["data_directory"]["path"].as_str().unwrap(),
        ]
    );
    let mut command = std::process::Command::new("rustfs");
    command.env_clear().envs(&validated.private);
    for key in [
        "RUSTFS_ACCESS_KEY_FILE",
        "rustfs_console_enable",
        "MINIO_ROOT_PASSWORD",
        "minio_access_key",
    ] {
        command.env(key, "forbidden");
    }
    storage_model::clear_overrides(&mut command);
    let retained = command
        .get_envs()
        .filter_map(|(key, value)| {
            value.map(|value| (key.to_str().unwrap(), value.to_str().unwrap()))
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        retained,
        std::collections::BTreeMap::from([
            ("PATH", "fixture-bin"),
            ("APP_ENV", "test"),
            ("CARGO_HOME", "fixture-cargo"),
            ("COREPACK_HOME", "fixture-corepack")
        ])
    );

    let mut changed = request.clone();
    changed["timeout_seconds"] = json!(true);
    assert!(
        storage_model::validate(
            &fixture.backend(),
            &run_dir,
            &manifest,
            CloneSide::Source,
            &changed
        )
        .is_err()
    );

    let credential = PathBuf::from(
        request["credential_files"]["secret_key"]["path"]
            .as_str()
            .unwrap(),
    );
    fs::write(&credential, b"changed").unwrap();
    assert!(
        storage_model::validate(
            &fixture.backend(),
            &run_dir,
            &manifest,
            CloneSide::Source,
            &request
        )
        .is_err()
    );
}

#[test]
fn storage_failure_identity_and_unknown_write_receipts_fail_closed() {
    let fixture = Fixture::new("storage-receipts");
    let (run_dir, manifest, request) = storage_fixture(&fixture);
    storage_model::validate(
        &fixture.backend(),
        &run_dir,
        &manifest,
        CloneSide::Source,
        &request,
    )
    .unwrap();
    let output = run_dir.join("storage-source/a0001");
    fs::create_dir_all(&output).unwrap();
    let registration = common::binding(&run_dir.join("storage-source/request.json")).unwrap();
    let controller = fixture.json("controller.json", &json!({"owner": "fixture"}));
    common::write_new(
        &output.join("intent.json"),
        &json!({
            "format_version": 1,
            "kind": "devex-clone-storage-intent",
            "request": registration,
            "controller": controller,
            "attempt": 1,
            "arguments": storage_model::arguments(&request).unwrap(),
        }),
    )
    .unwrap();
    assert!(
        storage::process::inspect_attempt(
            &fixture.backend(),
            &output,
            &registration,
            &controller,
            1
        )
        .is_err()
    );

    common::write_new(
        &output.join("failure-cleanup.json"),
        &json!({
            "intent": common::binding(&output.join("intent.json")).unwrap(),
            "identity": null,
            "pid": u32::MAX - 1,
            "returncode": 1,
        }),
    )
    .unwrap();
    let observed = storage::process::inspect_attempt(
        &fixture.backend(),
        &output,
        &registration,
        &controller,
        1,
    )
    .unwrap()
    .unwrap();
    assert_eq!(observed["state"], "not_started");

    let current = common::process_identity(std::process::id())
        .unwrap()
        .unwrap();
    let mut changed = current;
    changed["started"] = json!("changed");
    assert!(storage::process::running(&changed, &request, false).is_err());
}

#[test]
fn port_digest_and_link_failures_are_explicit() {
    let fixture = Fixture::new("policy");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    assert!(common::require_closed_port(&format!("http://127.0.0.1:{port}")).is_err());
    drop(listener);
    common::require_closed_port(&format!("http://127.0.0.1:{port}")).unwrap();

    let path = fixture.file("bound.json", br#"{"value":1}"#);
    let binding = common::binding(&path).unwrap();
    fs::write(&path, br#"{"value":2}"#).unwrap();
    assert!(common::read_bound(&fixture.backend(), &binding, "测试收据").is_err());

    let directory = fixture.path("real");
    fs::create_dir_all(&directory).unwrap();
    let link = fixture.path("linked");
    if create_directory_link(&directory, &link).is_ok() {
        assert!(common::local_directory(&fixture.backend(), &link, "链接目录").is_err());
    }
}

fn create_directory_link(source: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, target)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(source, target)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (source, target);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "link unsupported",
        ))
    }
}

#[test]
fn recorded_stop_reaps_exact_child_and_status_never_writes() {
    let fixture = Fixture::new("stop-status");
    let mut child = sleeper();
    let identity = wait_identity(child.id()).unwrap();
    let request = json!({
        "api_url": "http://127.0.0.1:1",
        "console_url": "http://127.0.0.1:2",
    });
    let output = fixture.path("stop");
    fs::create_dir_all(&output).unwrap();
    let observed = json!({
        "state": "recorded",
        "identity": identity,
        "process_receipt": {
            "path": output.join("process.json"),
            "bytes": 1,
            "sha256": "a".repeat(64),
        },
        "launch_receipt": null,
    });
    let result = storage::process::stop_record(&request, &observed, &output).unwrap();
    child.wait().unwrap();
    assert_eq!(result["state"], "stopped");
    assert_eq!(result["terminated"], true);
    assert!(common::process_identity(child.id()).unwrap().is_none());

    let run_dir = fixture.path("run");
    fs::create_dir_all(&run_dir).unwrap();
    common::write_new(
        &run_dir.join("manifest.json"),
        &json!({"format_version": 1}),
    )
    .unwrap();
    state::initialize(&run_dir).unwrap();
    let before = snapshot(&run_dir);
    let result = storage::status(&fixture.backend(), &run_dir, CloneSide::Source).unwrap();
    assert_eq!(result["status"], "storage_unregistered");
    assert_eq!(snapshot(&run_dir), before);
}

struct StorageParts {
    run_dir: PathBuf,
    data: PathBuf,
    executable: PathBuf,
    credentials: Value,
    environment: Value,
    source: Value,
    manifest: Value,
    api: String,
    console: String,
}

fn storage_parts(fixture: &Fixture) -> StorageParts {
    let run_dir = fixture.path("run");
    fs::create_dir_all(&run_dir).unwrap();
    let data = fixture.path("data");
    fs::create_dir_all(&data).unwrap();
    let executable = fixture.file("rustfs.exe", b"fixture-rustfs");
    let access = fixture.file("access.txt", b"fixture-access");
    let secret = fixture.file("secret.txt", b"fixture-secret");
    let credentials = json!({
        "access_key": common::binding(&access).unwrap(),
        "secret_key": common::binding(&secret).unwrap(),
    });
    let private = json!({
        "environment": {
            "PATH": "fixture-bin",
            "APP_ENV": "test",
            "CARGO_HOME": "fixture-cargo",
            "COREPACK_HOME": "fixture-corepack",
            "RUSTFS_UNDECLARED": "forbidden",
        }
    });
    let environment = fixture.json("environment.json", &private);
    let api = free_url();
    let console = free_url();
    let source_value = json!({
        "source": {
            "s3": {
                "endpoint": api,
                "access_key": "fixture-access",
                "secret_key": "fixture-secret",
            }
        }
    });
    let source = fixture.json("source.json", &source_value);
    let initialized = fixture.json("initialized.json", &json!({"fixture": true}));
    let manifest_value = json!({
        "source_request": source,
        "initialized": initialized,
        "source_environment": environment,
        "target_environment": environment,
    });
    let _manifest = fixture.json("run/manifest.json", &manifest_value);
    StorageParts {
        run_dir,
        data,
        executable,
        credentials,
        environment,
        source,
        manifest: manifest_value,
        api,
        console,
    }
}

#[cfg(any(unix, windows))]
fn target_storage_fixture(
    fixture: &Fixture,
    backend: &Path,
    executable: &Path,
) -> (PathBuf, Value, Value) {
    use crate::recovery::clone::native::digest;

    let StorageParts {
        run_dir,
        data,
        credentials,
        environment,
        source,
        mut manifest,
        api,
        console,
        ..
    } = storage_parts(fixture);
    let (device, inode) = device_inode(&data);
    let executable = common::binding(executable).unwrap();
    let identity = json!({"pid":610001,"started":"100","executable":executable["path"]});
    let mut request = json!({
        "format_version":1,"kind":"devex-clone-storage-restart","side":"target",
        "environment":environment,"scope_id":"storage-fixture","executable":executable,
        "data_directory":{"path":data,"device":device,"inode":inode},
        "api_url":api,"console_url":console,"credential_files":credentials,"timeout_seconds":5
    });
    let service =
        json!({"scope_id":request["scope_id"],"data_dir":data,"api":api,"console":console});
    let process = fixture.json(
        "target-process.json",
        &json!({
            "scope_id":request["scope_id"],"data_dir":data,"identity":identity,"lifecycle":"running"
        }),
    );
    let launch = fixture.json(
        "target-launch.json",
        &json!({
            "format_version":1,"scope_id":request["scope_id"],"identity":identity,
            "arguments":storage_model::arguments(&request).unwrap(),"credential_files":credentials
        }),
    );
    request["previous"] = json!({
        "identity":identity,"process_receipt":process,"launch_receipt":launch,"ready_receipt":process
    });

    let target = common::read_bound(backend, &source, "fixture source").unwrap()["source"].clone();
    let review = json!({"scopes":{"seed":target},"services":{"rustfs":service}});
    let mut review_binding = fixture.json("target-review.json", &review);
    review_binding["canonical_sha256"] = json!(digest::plan_hash(&review).unwrap());
    let original = json!({
        "side":"seed","review":review_binding,"target":target,
        "storage":{"rustfs":{"identity":identity,"sha256":executable["sha256"],
            "process_receipt":process,"launch_receipt":launch}}
    });
    fs::create_dir(fixture.path("target")).unwrap();
    let original_binding = fixture.json("target/request.json", &original);
    let prepare = fixture.json(
        "target/prepare.json",
        &json!({
            "request":original_binding,"request_sha256":digest::plan_hash(&original).unwrap()
        }),
    );
    let initialized = fixture.json(
        "target/initialized.json",
        &json!({
            "status":"fresh_target_initialized","prepare_sha256":prepare["sha256"]
        }),
    );
    manifest["initialized"] = initialized.clone();
    fs::write(
        run_dir.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    request["manifest"] = common::binding(&run_dir.join("manifest.json")).unwrap();
    request["original"] = initialized;
    (run_dir, manifest, request)
}

fn storage_fixture(fixture: &Fixture) -> (PathBuf, Value, Value) {
    let StorageParts {
        run_dir,
        data,
        executable,
        credentials,
        environment,
        source,
        manifest,
        api,
        console,
    } = storage_parts(fixture);
    let executable_binding = common::binding(&executable).unwrap();
    let old_identity = json!({
        "pid": 610001,
        "started": "100",
        "executable": executable,
    });
    let request_base = json!({
        "scope_id": "storage-fixture",
        "data_directory": {
            "path": data,
            "device": device_inode(&data).0,
            "inode": device_inode(&data).1,
        },
        "api_url": api,
        "console_url": console,
        "credential_files": credentials,
        "timeout_seconds": 5,
        "executable": executable_binding,
    });
    let launch = fixture.json(
        "launch.json",
        &json!({
            "scope_id": "storage-fixture",
            "executable": executable_binding,
            "data_dir": data,
            "api_url": api,
            "console_url": console,
            "access_key_file": credentials["access_key"]["path"],
            "secret_key_file": credentials["secret_key"]["path"],
        }),
    );
    let process = fixture.json(
        "process.json",
        &json!({
            "scope_id": "storage-fixture",
            "identity": old_identity,
            "data_dir": data,
            "launch_plan": launch,
            "command": storage_model::arguments(&request_base).unwrap(),
            "lifecycle": "started_unready",
            "secret_files": {
                "access_key_file": credentials["access_key"],
                "secret_key_file": credentials["secret_key"],
            }
        }),
    );
    let ready = fixture.json(
        "ready.json",
        &json!({
            "status": "ready",
            "identity": old_identity,
            "process_receipt": process,
            "data_dir": data,
            "listeners": [api, console],
        }),
    );
    let request = json!({
        "format_version": 1,
        "kind": "devex-clone-storage-restart",
        "side": "source",
        "manifest": common::binding(&run_dir.join("manifest.json")).unwrap(),
        "original": source,
        "environment": environment,
        "scope_id": "storage-fixture",
        "executable": executable_binding,
        "data_directory": request_base["data_directory"],
        "api_url": api,
        "console_url": console,
        "credential_files": credentials,
        "previous": {
            "identity": old_identity,
            "process_receipt": process,
            "launch_receipt": launch,
            "ready_receipt": ready,
        },
        "timeout_seconds": 5,
    });
    fs::create_dir_all(run_dir.join("storage-source")).unwrap();
    common::write_new(&run_dir.join("storage-source/request.json"), &request).unwrap();
    (run_dir, manifest, request)
}

fn free_url() -> String {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

fn wait_identity(pid: u32) -> Option<Value> {
    for _ in 0..100 {
        if let Some(value) = common::process_identity(pid).unwrap() {
            return Some(value);
        }
        thread::sleep(Duration::from_millis(10));
    }
    None
}

fn sleeper() -> std::process::Child {
    #[cfg(windows)]
    let mut command = Command::new(
        PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe"),
    );
    #[cfg(windows)]
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "Start-Sleep -Seconds 30",
    ]);
    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        command
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else {
                result.insert(entry.path(), fs::read(entry.path()).unwrap());
            }
        }
    }
    result
}

fn device_inode(path: &Path) -> (u64, u64) {
    let _metadata = fs::metadata(path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (_metadata.dev(), _metadata.ino())
    }
    #[cfg(windows)]
    {
        common::device_inode_for(path, &_metadata).unwrap()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = metadata;
        (0, 0)
    }
}
