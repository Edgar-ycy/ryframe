use super::*;
use crate::mysql_verification_client::{
    MYSQL_TIMEOUT_PARTIAL_STDOUT, MYSQL_VERIFICATION_STDERR, MysqlTimeoutTransport,
    assert_mysql_ownership_retry, compile_mysql_timeout_client, compile_mysql_verification_client,
};
use native::export::transport::{
    CommandOutput, CommandSpec, CommandTimeout, StdoutTarget, SystemTransport, Transport,
};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, collections::VecDeque, fs, io::Write, path::PathBuf};

struct Fixture {
    _directory: tempfile::TempDir,
    backend: PathBuf,
    plan_path: PathBuf,
    plan: Value,
    work: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        Self::with_mysql_verification_client(None)
    }

    fn with_mysql_verification_client(compile_client: Option<fn(&Path) -> PathBuf>) -> Self {
        let directory = super::super::tests::temporary();
        let root = directory.path();
        let backend = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_owned();
        let work = root.join("work 中文");
        let mut plan = json!({"id":"readonly-check","work_dir":work,"target_side":"base",
            "source":{"scope_id":"source","databases":[{"key":"control","kind":"combined","mode":"shared","database":"source_control","server_uuid":"uuid-1"}]},
            "target":{"scope_id":"target","databases":[{"key":"control","kind":"combined","mode":"shared","database":"target_control","server_uuid":"uuid-1"}]}});
        super::super::tests::formalize(&mut plan, root);
        if let Some(compile) = compile_client {
            let mysql = compile(root);
            plan["tools"]["mysql"] =
                json!({"path":mysql,"sha256":native::digest::file(&mysql).unwrap().sha256});
        }
        let config = root.join("config/app.test.toml");
        fs::write(&config, "[object_storage]\nendpoint='http://127.0.0.1:9000'\nregion='us-east-1'\naccess_key='fixture-access'\nsecret_key='fixture-secret'\n").unwrap();
        let digest = native::digest::file(&config).unwrap();
        let defaults = path(&plan["source"]["databases"][0]["defaults_file"]).unwrap();
        fs::write(defaults, "[client]\nhost=127.0.0.1\nport=3307\nuser=fixture\npassword=synthetic-secret\nssl-mode=REQUIRED\nssl-ca=synthetic-ca.pem\n").unwrap();
        let sha = native::digest::file(defaults).unwrap().sha256;
        for side in ["source", "target"] {
            let binding = &mut plan[side]["s3"]["configuration"];
            binding["files"]["app.test.toml"] =
                json!({"bytes":digest.bytes,"sha256":digest.sha256});
            let unsigned = json!({"app_env":binding["app_env"],"directory":binding["directory"],"files":binding["files"]});
            binding["sha256"] = json!(native::digest::plan_hash(&unsigned).unwrap());
            plan[side]["databases"][0]["defaults_sha256"] = json!(sha);
            let runtime = path(&plan[side]["runtime_dir"]).unwrap();
            fs::create_dir(runtime).unwrap();
            native::json::write_new(
                &runtime.join("api.json"),
                &json!({"format_version":1,
                "role":"api","scope_id":side,"identity":{"pid":123,"started":"fixture-start","executable":plan["tools"]["mysql"]["path"]}}),
            )
            .unwrap();
        }
        files::work(&json!({"plan":plan}), true).unwrap();
        let plan_path = root.join("plan.json");
        native::json::write_new(&plan_path, &plan).unwrap();
        Self {
            _directory: directory,
            backend,
            plan_path,
            plan,
            work,
        }
    }

    fn inputs(&self, side: &str) -> binding::Inputs {
        binding::Inputs::capture(&self.backend, &self.plan_path, &self.plan, &self.work, side)
            .unwrap()
    }
}

type Response = (bool, Vec<u8>, Vec<u8>);
type Call = (Vec<String>, Option<Vec<u8>>);

struct FakeTransport {
    responses: RefCell<VecDeque<Response>>,
    calls: RefCell<Vec<Call>>,
}

impl FakeTransport {
    fn new(rows: &[&str]) -> Self {
        Self {
            responses: RefCell::new(
                rows.iter()
                    .map(|row| (true, row.as_bytes().to_vec(), Vec::new()))
                    .collect(),
            ),
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl Transport for FakeTransport {
    fn run(&self, mut spec: CommandSpec) -> Result<CommandOutput> {
        self.calls.borrow_mut().push((spec.args, spec.stdin));
        let (success, raw, stderr) = self
            .responses
            .borrow_mut()
            .pop_front()
            .expect("意外重试或多余查询");
        let stdout = match &mut spec.stdout {
            StdoutTarget::HeldFile { file, .. } => {
                file.write_all(&raw)?;
                Vec::new()
            }
            StdoutTarget::Capture => raw,
            _ => panic!("只读检查不得使用未持有路径输出"),
        };
        #[cfg(windows)]
        let status = {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(u32::from(!success))
        };
        #[cfg(unix)]
        let status = {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(i32::from(!success) << 8)
        };
        Ok(CommandOutput {
            status,
            stdout,
            stderr,
        })
    }
    fn s3(&self, _: native::s3::Request) -> Result<Value> {
        panic!("两只读入口不得读取对象存储")
    }
}

#[test]
fn selected_side_and_source_empty_check_preserve_readonly_contract() {
    let fixture = Fixture::new();
    for (side, empty) in [("source", false), ("target", false), ("source", true)] {
        let identity = format!("uuid-1\t{side}_control\n");
        let owner = format!(
            "control\t{side}\tryframe-owner:v1:{side}:control\ntenant-data\t{side}\tryframe-owner:v1:{side}:tenant-data\ncontrol\t{side}\tryframe-owner:v1:{side}:control\n"
        );
        let mut rows = vec![identity.as_str(), owner.as_str()];
        if empty {
            rows.push("0\n");
        }
        let transport = FakeTransport::new(&rows);
        let inputs = fixture.inputs(side);
        let tools = collector::Collector {
            inputs: &inputs,
            transport: &transport,
        };
        observe(&fixture.plan, side, empty, &tools, |identity, url| {
            assert_eq!(identity["pid"], 123);
            assert_eq!(identity["started"], "fixture-start");
            assert_eq!(url, "http://127.0.0.1:3000");
            Ok(())
        })
        .unwrap();
        let calls = transport.calls.borrow();
        assert_eq!(calls.len(), if empty { 3 } else { 2 });
        for (index, (args, stdin)) in calls.iter().enumerate() {
            assert!(args.contains(&format!("--database={side}_control")));
            if index < 2 {
                assert!(stdin.is_none());
                assert_eq!(args[args.len() - 2], "--execute");
            } else {
                assert_eq!(
                    stdin.as_deref(),
                    Some(
                        b"SELECT COUNT(*) FROM sys_tenant WHERE tenant_id <> 'system';".as_slice()
                    )
                );
            }
        }
        inputs.unchanged().unwrap();
    }
}

#[test]
fn mysql_reference_rejects_unsupported_selector_before_capture_or_transport() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs("source");
    let transport = FakeTransport::new(&[]);
    let tools = collector::Collector {
        inputs: &inputs,
        transport: &transport,
    };
    let before = fs::read_dir(&fixture.work).unwrap().count();
    for selector in ["", "IDENTITY", "ownership\n", "SELECT 1;"] {
        let error = tools
            .query(&fixture.plan["source"]["databases"][0], selector)
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "数据库核验只允许固定 identity 或 ownership 查询"
        );
    }
    assert!(transport.calls.borrow().is_empty());
    assert_eq!(fs::read_dir(&fixture.work).unwrap().count(), before);
}

#[test]
fn mysql_reference_ownership_retries_once_and_binds_both_outputs() {
    let ownership = concat!(
        "control\tsource\tryframe-owner:v1:source:control\n",
        "tenant-data\tsource\tryframe-owner:v1:source:tenant-data\n"
    );
    for second in [ownership, ""] {
        let fixture = Fixture::new();
        let inputs = fixture.inputs("source");
        let transport = FakeTransport::new(&["", second]);
        let tools = collector::Collector {
            inputs: &inputs,
            transport: &transport,
        };
        let result = tools.query(&fixture.plan["source"]["databases"][0], "ownership");
        if second.is_empty() {
            assert_eq!(
                result.unwrap_err().to_string(),
                "MySQL 固定核验第二次仍为空；保留两次输出，禁止继续重试"
            );
        } else {
            assert_eq!(result.unwrap(), ownership.trim_end());
        }
        let calls = transport.calls.borrow();
        assert_eq!(calls.len(), 2);
        for (args, stdin) in calls.iter() {
            assert!(stdin.is_none());
            assert_eq!(
                args[args.len() - 2..],
                [
                    "--execute",
                    "SELECT resource_kind, scope_id, marker FROM ryframe_resource_ownership ORDER BY resource_kind;",
                ]
            );
        }
        assert_mysql_ownership_retry(&fixture.work, "control", second.as_bytes());
    }
}

#[test]
fn empty_retry_and_failed_second_attempt_keep_exact_output_and_relationship() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs("source");
    let transport = FakeTransport::new(&["", "partial-sensitive"]);
    transport.responses.borrow_mut()[1].0 = false;
    let tools = collector::Collector {
        inputs: &inputs,
        transport: &transport,
    };
    let error = tools
        .query(&fixture.plan["source"]["databases"][0], "identity")
        .unwrap_err();
    assert!(!error.to_string().contains("partial-sensitive"));
    assert_eq!(transport.calls.borrow().len(), 2);
    let records = fs::read_dir(&fixture.work)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| native::json::read(&path).unwrap())
        .collect::<Vec<_>>();
    let retry = records
        .iter()
        .find(|value| value["kind"] == "mysql-verification-retry")
        .unwrap();
    for name in ["first", "second"] {
        let receipt = native::json::read(path(&retry[name]["path"]).unwrap()).unwrap();
        let output = path(&receipt["stdout_file"]["path"]).unwrap();
        assert_eq!(
            native::digest::file(output).unwrap().sha256,
            receipt["stdout_file"]["sha256"]
        );
    }
    assert!(
        records
            .iter()
            .any(|value| value["returncode"] == 1 && value["error_type"] == "CommandFailed")
    );
}

#[test]
fn mysql_reference_stderr_digest_uses_raw_child_bytes_without_retry() {
    let fixture = Fixture::with_mysql_verification_client(Some(compile_mysql_verification_client));
    let inputs = fixture.inputs("source");
    let collector = collector::Collector {
        inputs: &inputs,
        transport: &SystemTransport,
    };
    assert!(
        verify_database(
            &fixture.plan,
            "source",
            &fixture.plan["source"]["databases"][0],
            &collector
        )
        .is_err()
    );
    let receipts = fs::read_dir(&fixture.work)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("mysql-")
                && path.extension().is_some_and(|ext| ext == "json")
        })
        .collect::<Vec<_>>();
    assert_eq!(receipts.len(), 1);
    let mysql = native::json::read(&receipts[0]).unwrap();
    assert_eq!(mysql["kind"], "mysql-verification-output");
    assert_eq!(mysql["returncode"], 0);
    assert!(mysql["error_type"].is_null());
    assert_eq!(mysql["stderr_bytes"], MYSQL_VERIFICATION_STDERR.len());
    assert_eq!(
        mysql["stderr_sha256"],
        hex::encode(Sha256::digest(MYSQL_VERIFICATION_STDERR))
    );
    assert_eq!(mysql["stdout_file"]["bytes"], 0);
    assert!(!mysql.to_string().contains("mysql-verification-secret"));
}

#[test]
fn mysql_reference_timeout_keeps_typed_failure_and_partial_stdout() {
    let fixture = Fixture::with_mysql_verification_client(Some(compile_mysql_timeout_client));
    let inputs = fixture.inputs("source");
    let transport = MysqlTimeoutTransport::default();
    let collector = collector::Collector {
        inputs: &inputs,
        transport: &transport,
    };
    let error = collector
        .query(&fixture.plan["source"]["databases"][0], "identity")
        .unwrap_err();
    assert!(
        error
            .downcast_ref::<CommandTimeout>()
            .unwrap()
            .tree_reclaimed()
    );
    assert!(!error.to_string().contains("partial-private-mysql"));
    assert_eq!(transport.calls.get(), 1);
    let receipts = fs::read_dir(&fixture.work)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("mysql-")
                && path.extension().is_some_and(|ext| ext == "json")
        })
        .collect::<Vec<_>>();
    let [receipt]: [PathBuf; 1] = receipts.try_into().unwrap();
    let mysql = native::json::read(&receipt).unwrap();
    assert_eq!(mysql["error_type"], "TimeoutExpired");
    assert!(mysql["returncode"].is_null());
    assert!(mysql["stderr_sha256"].is_null());
    assert!(mysql["stderr_bytes"].is_null());
    let stdout = path(&mysql["stdout_file"]["path"]).unwrap();
    assert_eq!(fs::read(stdout).unwrap(), MYSQL_TIMEOUT_PARTIAL_STDOUT);
    assert_eq!(
        mysql["stdout_file"]["bytes"],
        MYSQL_TIMEOUT_PARTIAL_STDOUT.len()
    );
    assert_eq!(
        mysql["stdout_file"]["sha256"],
        hex::encode(Sha256::digest(MYSQL_TIMEOUT_PARTIAL_STDOUT))
    );
}

#[test]
fn malformed_nonempty_and_stderr_do_not_retry_and_bound_inputs_cannot_drift() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs("source");
    for (raw, stderr) in [("", "synthetic-secret"), ("invalid-identity", "")] {
        let transport = FakeTransport::new(&[raw]);
        transport.responses.borrow_mut()[0].2 = stderr.as_bytes().to_vec();
        let tools = collector::Collector {
            inputs: &inputs,
            transport: &transport,
        };
        assert!(
            verify_database(
                &fixture.plan,
                "source",
                &fixture.plan["source"]["databases"][0],
                &tools
            )
            .is_err()
        );
        assert_eq!(transport.calls.borrow().len(), 1);
    }
    let invalid_utf8 = FakeTransport::new(&[""]);
    invalid_utf8.responses.borrow_mut()[0].1 = vec![0xff];
    let tools = collector::Collector {
        inputs: &inputs,
        transport: &invalid_utf8,
    };
    assert!(
        tools
            .query(&fixture.plan["source"]["databases"][0], "identity")
            .is_err()
    );
    assert_eq!(invalid_utf8.calls.borrow().len(), 1);
    assert!(
        fs::read_dir(&fixture.work)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path
                .extension()
                .is_some_and(|extension| extension == "json"))
            .map(|path| native::json::read(&path).unwrap())
            .any(|value| value["error_type"] == "UnicodeDecodeError")
    );
    fs::write(&inputs.registration, b"{}").unwrap();
    let transport = FakeTransport::new(&[]);
    let tools = collector::Collector {
        inputs: &inputs,
        transport: &transport,
    };
    assert!(
        tools
            .query(&fixture.plan["source"]["databases"][0], "identity")
            .is_err()
    );
    assert!(transport.calls.borrow().is_empty());
}

#[test]
fn ownership_and_registered_process_scope_mismatch_never_reach_listener_success() {
    for wrong_owner in [true, false] {
        let fixture = Fixture::new();
        let scope = if wrong_owner { "other" } else { "source" };
        let owners = format!(
            "control\t{scope}\tryframe-owner:v1:{scope}:control\ntenant-data\t{scope}\tryframe-owner:v1:{scope}:tenant-data"
        );
        if !wrong_owner {
            let registration = path(&fixture.plan["source"]["runtime_dir"])
                .unwrap()
                .join("api.json");
            let mut receipt = native::json::read(&registration).unwrap();
            receipt["scope_id"] = json!("other");
            fs::write(registration, serde_json::to_vec(&receipt).unwrap()).unwrap();
        }
        let inputs = fixture.inputs("source");
        let transport = FakeTransport::new(&["uuid-1\tsource_control", &owners]);
        let tools = collector::Collector {
            inputs: &inputs,
            transport: &transport,
        };
        assert!(
            observe(&fixture.plan, "source", false, &tools, |_, _| panic!(
                "跨 scope 观察不能到达监听成功路径"
            ))
            .is_err()
        );
        assert_eq!(transport.calls.borrow().len(), 2);
    }
}

#[test]
fn source_tenant_count_is_not_retried_and_nonempty_source_is_rejected() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs("source");
    let transport = FakeTransport::new(&[
        "uuid-1\tsource_control",
        "control\tsource\tryframe-owner:v1:source:control\ntenant-data\tsource\tryframe-owner:v1:source:tenant-data",
        "1",
    ]);
    let tools = collector::Collector {
        inputs: &inputs,
        transport: &transport,
    };
    assert!(observe(&fixture.plan, "source", true, &tools, |_, _| Ok(())).is_err());
    assert_eq!(transport.calls.borrow().len(), 3);
}

#[test]
fn client_defaults_preserve_tls_fields_and_reject_unsafe_options_without_secret_diagnostics() {
    let fixture = Fixture::new();
    let defaults = path(&fixture.plan["source"]["databases"][0]["defaults_file"]).unwrap();
    let valid = "[DEFAULT]\n[client]\nHOST: 127.0.0.1\nPORT: 3307\nuser: fixture\npassword: synthetic-secret\nssl-ca: local ca.pem\nssl-cert: local cert.pem\nssl-key: local key.pem\n";
    fs::write(defaults, valid).unwrap();
    defaults::validate(defaults).unwrap();
    for invalid in [
        valid.replace("127.0.0.1", "remote.invalid"),
        valid.replace("PORT: 3307\n", ""),
        valid.replace("user: fixture", "user: "),
        format!("{valid}socket=synthetic-secret\n"),
        format!("{valid}PASSWORD=second\n"),
        format!("{valid}[other]\n"),
        valid.replace("[DEFAULT]", "[DEFAULT]\npassword=synthetic-secret"),
    ] {
        fs::write(defaults, invalid).unwrap();
        let error = defaults::validate(defaults).unwrap_err().to_string();
        assert!(!error.contains("synthetic-secret"));
    }
}
