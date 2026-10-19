use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    fs,
    path::Path,
    process::ExitStatus,
};

use serde_json::{Value, json};

use crate::recovery::clone::native::{
    digest,
    export::transport::{CommandOutput, CommandSpec, Transport},
    json as json_io,
    resources::common,
    target_verify::{
        input::TargetContext,
        storage::{StorageObserver, generation, kernel, redis_layout, rustfs},
    },
};

struct Fixture {
    _directory: tempfile::TempDir,
    context: TargetContext,
    expected: Value,
    storage: Value,
    launch: Value,
    identity: Value,
}

impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let parent = backend.join(".local-tests");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("target-storage-model-")
            .tempdir_in(parent)
            .unwrap();
        let root = directory.path();
        let data = root.join("中文 空格.data");
        fs::create_dir(&data).unwrap();
        let output = root.join("observed");
        fs::create_dir(&output).unwrap();
        let identity = common::process_identity(std::process::id())
            .unwrap()
            .unwrap();
        let executable = identity["executable"].as_str().unwrap();
        let expected = json!({"scope_id":"storage-fixture", "data_dir":data,
            "api":"http://127.0.0.1:29200", "console":"http://127.0.0.1:29201",
            "process_receipt":root.join("process.json")});
        let mut credentials = json!({});
        for (key, value) in [("access_key", "access"), ("secret_key", "secret")] {
            let path = root.join(format!("{key}.txt"));
            fs::write(&path, format!("  {value}\n")).unwrap();
            credentials[key] = common::binding(&path).unwrap();
        }
        let launch = json!({"format_version":1, "scope_id":"storage-fixture", "identity":identity,
            "arguments":rustfs::arguments(&expected, &identity, &data).unwrap(),
            "environment":{"RUSTFS_CONSOLE_ENABLE":"true",
                "RUSTFS_ACCESS_KEY_FILE":credentials["access_key"]["path"],
                "RUSTFS_SECRET_KEY_FILE":credentials["secret_key"]["path"]}, "credential_files":credentials});
        let process = json!({"format_version":1, "role":"rustfs", "scope_id":"storage-fixture",
            "lifecycle":"running", "identity":identity, "api_url":expected["api"],
            "console_url":expected["console"], "data_dir":data});
        json_io::write_new(&root.join("process.json"), &process).unwrap();
        json_io::write_new(&root.join("launch.json"), &launch).unwrap();
        let storage = json!({"identity":identity, "sha256":digest::file(Path::new(executable)).unwrap().sha256,
            "process_receipt":common::binding(&root.join("process.json")).unwrap(),
            "launch_receipt":common::binding(&root.join("launch.json")).unwrap()});
        let context = context(&backend, root, &output);
        Self {
            _directory: directory,
            context,
            expected,
            storage,
            launch,
            identity,
        }
    }

    fn arguments(&self) -> Vec<String> {
        serde_json::from_value(self.launch["arguments"].clone()).unwrap()
    }
}

pub(super) fn context(backend: &Path, root: &Path, output: &Path) -> TargetContext {
    TargetContext {
        backend: backend.into(),
        run_dir: root.into(),
        manifest: json!({}),
        target_dir: root.into(),
        initialized_path: root.join("initialized.json"),
        initialized_binding: json!({}),
        initialized: json!({}),
        request_path: root.join("request.json"),
        request: json!({"target":{"s3":{"access_key_env":"FIXTURE_ACCESS", "secret_key_env":"FIXTURE_SECRET"}}}),
        environment: BTreeMap::from([
            ("FIXTURE_ACCESS".into(), "access".into()),
            ("FIXTURE_SECRET".into(), "secret".into()),
            ("SystemRoot".into(), "D:\\Windows".into()),
        ]),
        observation_dir: output.into(),
    }
}

#[test]
fn rustfs_process_receipt_checks_every_fixed_field() {
    let fixture = Fixture::new();
    let receipt = json_io::read(&fixture.context.target_dir.join("process.json")).unwrap();
    rustfs::validate_receipt(&receipt, &fixture.expected, &fixture.identity).unwrap();
    for key in [
        "format_version",
        "role",
        "scope_id",
        "lifecycle",
        "identity",
        "api_url",
        "console_url",
        "data_dir",
    ] {
        let mut changed = receipt.clone();
        changed[key] = Value::Null;
        assert!(
            rustfs::validate_receipt(&changed, &fixture.expected, &fixture.identity).is_err(),
            "{key}"
        );
    }
}

#[test]
fn rustfs_launch_rejects_unknown_fields_and_credential_or_environment_changes() {
    let fixture = Fixture::new();
    let check = |launch: &Value| {
        rustfs::validate_launch(
            &fixture.context,
            &fixture.expected,
            &fixture.identity,
            launch,
            &fixture.arguments(),
        )
    };
    check(&fixture.launch).unwrap();
    for pointer in [
        "/format_version",
        "/scope_id",
        "/identity",
        "/arguments",
        "/environment",
    ] {
        let mut changed = fixture.launch.clone();
        *changed.pointer_mut(pointer).unwrap() = Value::Null;
        assert!(check(&changed).is_err(), "{pointer}");
    }
    let mut extra = fixture.launch.clone();
    extra["extra"] = json!(true);
    assert!(check(&extra).is_err());
    let mut credentials = fixture.launch.clone();
    credentials["credential_files"]["unexpected"] = json!({});
    assert!(check(&credentials).is_err());
    let path = Path::new(
        fixture.launch["credential_files"]["secret_key"]["path"]
            .as_str()
            .unwrap(),
    );
    fs::write(path, "changed-private-value").unwrap();
    credentials = fixture.launch.clone();
    credentials["credential_files"]["secret_key"] = common::binding(path).unwrap();
    let error = check(&credentials).unwrap_err().to_string();
    assert!(!error.contains("changed-private-value"));
    assert!(!error.contains("secret"));
}

#[test]
fn windows_arguments_keep_empty_unicode_space_quotes_and_backslashes() {
    for (raw, expected) in [
        (
            r#""D:\中文 空格\rustfs.exe" server "D:\data dir\\""#,
            vec![r"D:\中文 空格\rustfs.exe", "server", "D:\\data dir\\"],
        ),
        (r#"tool "" "a b" a\"b"#, vec!["tool", "", "a b", "a\"b"]),
        ("tool\tserver", vec!["tool", "server"]),
        (" tool", vec!["", "tool"]),
        (r#"tool "a""b""#, vec!["tool", "a\"b"]),
    ] {
        assert_eq!(kernel::windows_argv(raw).unwrap(), expected, "{raw}");
    }
    assert!(kernel::windows_argv("").is_err());
    assert!(kernel::windows_argv("tool\0server").is_err());
}

#[test]
fn rustfs_listener_requires_registered_ipv4_endpoint_owner() {
    let identity = json!({"pid":42});
    kernel::validate_listeners(
        &json!([{"LocalAddress":"127.0.0.1","OwningProcess":42}]),
        &identity,
    )
    .unwrap();
    for rows in [
        json!([]),
        json!({}),
        json!([{"LocalAddress":"127.0.0.1","OwningProcess":43}]),
        json!([{"LocalAddress":"::1","OwningProcess":42}]),
    ] {
        assert!(kernel::validate_listeners(&rows, &identity).is_err());
    }
    kernel::validate_listeners(
        &json!([{"LocalAddress":"127.0.0.1","OwningProcess":42}, {"LocalAddress":"::","OwningProcess":43}]),
        &identity,
    ).unwrap();
}

#[test]
fn storage_transition_only_accepts_registered_complete_replacements() {
    let expected = json!({"rustfs":{"identity":1},"redis":{"run_id":"old"}});
    let runtime = json!({"storage":{"identity":2}});
    let actual = json!({"rustfs":{"identity":2},"redis":{"run_id":"old"}});
    generation::validate_transition(&expected, &expected, None, None).unwrap();
    generation::validate_transition(&expected, &actual, Some(&runtime), None).unwrap();
    assert!(generation::validate_transition(&expected, &actual, None, None).is_err());
    let changed = json!({"rustfs":{"identity":2},"redis":{"run_id":"unregistered"}});
    assert!(generation::validate_transition(&expected, &changed, Some(&runtime), None).is_err());
}

#[test]
fn redis_info_rejects_duplicates_and_kernel_identity_drift() {
    assert!(redis_layout::info("process_id:200\nprocess_id:201\n").is_err());
    let redis = json!({"pid":200,"started":"67890","run_id":"3".repeat(40),
        "executable":"/usr/bin/redis-server","sha256":"9".repeat(64)});
    let stat = format!("200 (redis (server)) {} 67890", vec!["0"; 19].join(" "));
    let info = redis_layout::info(&format!(
        "# Server\r\nprocess_id:200\r\nrun_id:{}\r\n",
        "3".repeat(40)
    ))
    .unwrap();
    let hash = format!("{}  /usr/bin/redis-server", "9".repeat(64));
    redis_layout::validate_kernel(&redis, &stat, "/usr/bin/redis-server", &hash, &info).unwrap();
    for field in ["pid", "started", "run_id", "executable", "sha256"] {
        let mut changed = redis.clone();
        changed[field] = json!("changed");
        assert!(
            redis_layout::validate_kernel(&changed, &stat, "/usr/bin/redis-server", &hash, &info)
                .is_err(),
            "{field}"
        );
    }
    assert!(
        redis_layout::validate_kernel(&redis, "short", "/usr/bin/redis-server", &hash, &info)
            .is_err()
    );
}

#[test]
fn redis_configuration_reads_all_eight_fields_and_rejects_each_drift() {
    let directory = Path::new(r"D:\中文 空格\redis");
    let redis = json!({"port":16390});
    let expected = BTreeMap::from([
        ("bind", "127.0.0.1"),
        ("port", "16390"),
        ("dir", "/mnt/d/中文 空格/redis"),
        ("databases", "1"),
        ("protected-mode", "yes"),
        ("save", ""),
        ("appendonly", "no"),
        ("daemonize", "no"),
    ]);
    let mut visited = Vec::new();
    redis_layout::verify_configuration(&redis, directory, &mut |parts| {
        assert_eq!(&parts[..2], &["CONFIG", "GET"]);
        visited.push(parts[2].to_owned());
        Ok(json!([parts[2], expected[parts[2]]]))
    })
    .unwrap();
    assert_eq!(visited.len(), 8);
    for wrong in expected.keys() {
        assert!(
            redis_layout::verify_configuration(&redis, directory, &mut |parts| {
                Ok(json!([
                    parts[2],
                    if parts[2] == *wrong {
                        "changed"
                    } else {
                        expected[parts[2]]
                    }
                ]))
            })
            .is_err(),
            "{wrong}"
        );
    }
    for path in [r"\\server\share\redis", "/tmp/redis", "relative/redis"] {
        assert!(redis_layout::wsl_path(Path::new(path)).is_err());
    }
}

struct FakeTransport {
    output: String,
    calls: RefCell<Vec<String>>,
    failure: bool,
    listeners: Option<Value>,
}

impl Transport for FakeTransport {
    fn run(&self, spec: CommandSpec) -> crate::Result<CommandOutput> {
        self.calls.borrow_mut().push(spec.label.clone());
        assert!(spec.stdin.is_none());
        #[cfg(windows)]
        let status = {
            use std::os::windows::process::ExitStatusExt;
            ExitStatus::from_raw(u32::from(self.failure))
        };
        #[cfg(unix)]
        let status = {
            use std::os::unix::process::ExitStatusExt;
            ExitStatus::from_raw(i32::from(self.failure) << 8)
        };
        Ok(CommandOutput {
            status,
            stdout: if spec.label == "target-storage-listener" {
                serde_json::to_vec(self.listeners.as_ref().ok_or("缺少假监听结果")?)?
            } else {
                self.output.as_bytes().to_vec()
            },
            stderr: b"private-command-failure-body".to_vec(),
        })
    }
}

#[test]
fn kernel_transport_is_read_only_and_does_not_expose_command_failure_body() {
    let fixture = Fixture::new();
    let transport = FakeTransport {
        output: "fixed output".into(),
        calls: RefCell::new(Vec::new()),
        failure: false,
        listeners: None,
    };
    let value = kernel::observe(
        &fixture.context,
        &transport,
        "fixed-tool".into(),
        vec!["fixed-arg".into()],
        "storage-test",
    )
    .unwrap();
    assert_eq!(value, "fixed output");
    assert_eq!(transport.calls.borrow().as_slice(), &["storage-test"]);
    let failed = FakeTransport {
        output: "private-command-failure-body".into(),
        calls: RefCell::new(Vec::new()),
        failure: true,
        listeners: None,
    };
    let error = kernel::observe(
        &fixture.context,
        &failed,
        "fixed-tool".into(),
        Vec::new(),
        "storage-test",
    )
    .unwrap_err()
    .to_string();
    assert!(!error.contains("private-command-failure-body"));
}

#[test]
fn rustfs_fake_observer_checks_arguments_and_publishes_only_digest_evidence() {
    let fixture = Fixture::new();
    let args = fixture.arguments();
    let observer = FakeObserver {
        identity: fixture.identity.clone(),
        arguments: args.clone(),
        calls: RefCell::new(Vec::new()),
        raced: Cell::new(false),
        change_after_arguments: false,
    };
    let review = json!({"services":{"rustfs":fixture.expected},"tools":{"rustfs":{
        "path":fixture.identity["executable"],"sha256":fixture.storage["sha256"]}}});
    rustfs::verify(&fixture.context, &review, &fixture.storage, None, &observer).unwrap();
    assert_eq!(
        observer.calls.borrow().as_slice(),
        &[
            "target-storage-argv",
            "target-storage-listener",
            "target-storage-listener"
        ]
    );
    let evidence = fs::read_dir(&fixture.context.observation_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let observed = json_io::read(&evidence).unwrap();
    assert_eq!(
        observed["actual_arguments_sha256"],
        json!(digest::plan_hash(&json!(args)).unwrap())
    );
    assert!(observed.get("arguments").is_none());
    assert!(observed.get("credential_files").is_none());
    let mut changed = fixture.storage.clone();
    changed["identity"]["started"] = json!("0");
    let count = observer.calls.borrow().len();
    assert!(rustfs::verify(&fixture.context, &review, &changed, None, &observer).is_err());
    assert_eq!(observer.calls.borrow().len(), count);
}

struct FakeObserver {
    identity: Value,
    arguments: Vec<String>,
    calls: RefCell<Vec<String>>,
    raced: Cell<bool>,
    change_after_arguments: bool,
}

impl StorageObserver for FakeObserver {
    fn process_identity(&self, _pid: u32) -> crate::Result<Option<Value>> {
        let mut identity = self.identity.clone();
        if self.raced.get() {
            identity["started"] = json!("0");
        }
        Ok(Some(identity))
    }

    fn arguments(&self, _context: &TargetContext, _identity: &Value) -> crate::Result<Vec<String>> {
        self.calls.borrow_mut().push("target-storage-argv".into());
        self.raced.set(self.change_after_arguments);
        Ok(self.arguments.clone())
    }

    fn listener(
        &self,
        _context: &TargetContext,
        identity: &Value,
        _url: &str,
    ) -> crate::Result<()> {
        self.calls
            .borrow_mut()
            .push("target-storage-listener".into());
        kernel::validate_listeners(
            &json!([{"LocalAddress":"127.0.0.1","OwningProcess":self.identity["pid"]}]),
            identity,
        )
    }

    fn wsl(
        &self,
        _context: &TargetContext,
        _redis: &Value,
        _args: &[String],
    ) -> crate::Result<String> {
        Err("RustFS 测试不得调用未安排的 WSL 观察".into())
    }

    fn redis(
        &self,
        _context: &TargetContext,
        _redis: &Value,
        _parts: &[&str],
    ) -> crate::Result<Value> {
        Err("RustFS 测试不得调用未安排的 Redis 观察".into())
    }
}

#[test]
fn rustfs_observer_pid_race_and_argv_drift_stop_before_evidence() {
    let fixture = Fixture::new();
    let review = json!({"services":{"rustfs":fixture.expected},"tools":{"rustfs":{
        "path":fixture.identity["executable"],"sha256":fixture.storage["sha256"]}}});
    for race in [false, true] {
        let mut arguments = fixture.arguments();
        if !race {
            arguments.push("--unexpected".into());
        }
        let observer = FakeObserver {
            identity: fixture.identity.clone(),
            arguments,
            calls: RefCell::new(Vec::new()),
            raced: Cell::new(false),
            change_after_arguments: race,
        };
        assert!(
            rustfs::verify(&fixture.context, &review, &fixture.storage, None, &observer).is_err()
        );
        assert_eq!(observer.calls.borrow().as_slice(), &["target-storage-argv"]);
        assert_eq!(
            fs::read_dir(&fixture.context.observation_dir)
                .unwrap()
                .count(),
            0
        );
    }
}

#[test]
fn runtime_directory_identity_rejects_a_changed_inode() {
    let fixture = Fixture::new();
    let directory = Path::new(fixture.expected["data_dir"].as_str().unwrap());
    let runtime = json!({"storage":fixture.storage,"data_directory":{"path":directory,"device":0,"inode":0},
        "api_url":fixture.expected["api"],"console_url":fixture.expected["console"]});
    assert!(
        rustfs::validate_directory(
            &fixture.context.backend,
            &fixture.expected,
            &fixture.storage,
            Some(&runtime),
            directory
        )
        .is_err()
    );
    rustfs::validate_directory(
        &fixture.context.backend,
        &fixture.expected,
        &fixture.storage,
        None,
        directory,
    )
    .unwrap();
}
