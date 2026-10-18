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
        let root = workspace::root_dir().join(format!(
            ".local-tests/clone-resources-{name}-{}-{}",
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
    assert_eq!(validated.private["KEY"], "access-private");
    assert_eq!(storage_model::arguments(&request).unwrap()[1], "server");

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
            "environment": storage_model::configuration(&request).unwrap(),
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
    let access = fixture.file("access.txt", b"access-private");
    let secret = fixture.file("secret.txt", b"secret-private");
    let credentials = json!({
        "access_key": common::binding(&access).unwrap(),
        "secret_key": common::binding(&secret).unwrap(),
    });
    let private = json!({
        "environment": {
            "KEY": "access-private",
            "SECRET": "secret-private",
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
                "access_key_env": "KEY",
                "secret_key_env": "SECRET",
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
        use winsafe::{self as w, co, prelude::*};
        let (handle, _) = w::HFILE::CreateFile(
            path.to_str().unwrap(),
            co::GENERIC::default(),
            Some(co::FILE_SHARE::READ | co::FILE_SHARE::WRITE | co::FILE_SHARE::DELETE),
            None,
            co::DISPOSITION::OPEN_EXISTING,
            co::FILE_ATTRIBUTE::NORMAL,
            Some(co::FILE_FLAG::BACKUP_SEMANTICS),
            None,
            None,
        )
        .unwrap();
        let mut info = w::BY_HANDLE_FILE_INFORMATION::default();
        handle.GetFileInformationByHandle(&mut info).unwrap();
        (
            u64::from(info.dwVolumeSerialNumber),
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        )
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = metadata;
        (0, 0)
    }
}
