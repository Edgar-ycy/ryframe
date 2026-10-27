use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    fs::{self, File},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::ExitStatus,
    time::Duration,
};

use serde_json::{Value, json};

use super::{CommandFailure, Evidence, Port, mysql_receipt, stdout::VerifiedStdout};
use crate::{
    Result,
    recovery::clone::native::{
        digest,
        export::transport::{CommandOutput, CommandSpec, StdoutTarget, SystemTransport, Transport},
        json as json_io,
        target_verify::input::TargetContext,
    },
};

const RAW: &[u8] = b"private-stdout\tfixture\n";

struct Fixture {
    root: PathBuf,
    context: TargetContext,
    database: Value,
}

impl Fixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests")
            .join(format!("target-stdout-{}", uuid::Uuid::new_v4().simple()));
        let local = root.join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        fs::create_dir(root.join("config")).unwrap();
        fs::write(
            root.join("config/app.test.toml"),
            "[database.primary]\npassword=\"private-stdout\"\n",
        )
        .unwrap();
        let defaults = local.join("mysql.cnf");
        fs::write(&defaults, "[client]\nhost=127.0.0.1\nport=3306\nuser=fixture\npassword=private-stdout\nssl-mode=DISABLED\n").unwrap();
        let mysql = local.join("mysql");
        fs::write(&mysql, b"fixture-tool").unwrap();
        let database = json!({"key": "shared", "kind": "tenant", "mode": "shared", "database": "fixture", "server_uuid": "fixture-uuid", "defaults_file": defaults, "defaults_sha256": digest::file(&defaults).unwrap().sha256});
        let context = TargetContext {
            backend: root.clone(),
            run_dir: local.clone(),
            manifest: json!({}),
            target_dir: local.clone(),
            initialized_path: local.join("initialized.json"),
            initialized_binding: json!({}),
            initialized: json!({}),
            request_path: local.join("request.json"),
            request: json!({"config_dir":root.join("config"),"target": {"scope_id": "stdout-fixture", "databases": [database]}, "tools": {"mysql": {"path": mysql, "sha256": digest::file(&mysql).unwrap().sha256}}}),
            environment: BTreeMap::from([("APP_ENV".into(), "test".into())]),
            observation_dir: local,
        };
        Self {
            root,
            context,
            database,
        }
    }

    fn files(&self, prefix: &str) -> Vec<PathBuf> {
        let mut paths = fs::read_dir(&self.context.observation_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with(prefix)
            })
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }

    fn system_spec(&self, stdout: StdoutTarget, code: &str) -> CommandSpec {
        CommandSpec {
            program: crate::process::resolved_executable(
                Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap(),
                "python",
            )
            .unwrap()
            .into(),
            args: vec!["-B".into(), "-c".into(), code.into()],
            cwd: self.root.clone(),
            environment: std::env::vars().collect(),
            stdin: None,
            stdout,
            work_dir: self.context.observation_dir.clone(),
            label: "原始输出句柄测试".into(),
            timeout: Duration::from_secs(60),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn write_held(output: &VerifiedStdout, bytes: &[u8]) {
    let StdoutTarget::HeldFile { mut file, .. } = output.target().unwrap() else {
        panic!("必须使用持有句柄");
    };
    file.seek(SeekFrom::Start(0)).unwrap();
    file.set_len(0).unwrap();
    file.write_all(bytes).unwrap();
    file.flush().unwrap();
}

#[cfg(windows)]
fn status(code: u32) -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(code)
}
#[cfg(unix)]
fn status(code: u32) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw((code as i32) << 8)
}

struct Fake {
    calls: Cell<usize>,
    code: u32,
    transport_error: bool,
    writer: RefCell<Option<File>>,
}

impl Fake {
    fn new(code: u32, transport_error: bool) -> Self {
        Self {
            calls: Cell::new(0),
            code,
            transport_error,
            writer: RefCell::new(None),
        }
    }
}

impl Transport for Fake {
    fn s3(
        &self,
        _: crate::recovery::clone::native::s3::Request,
    ) -> crate::Result<serde_json::Value> {
        panic!("该测试不允许对象存储调用")
    }

    fn run(&self, spec: CommandSpec) -> Result<CommandOutput> {
        self.calls.set(self.calls.get() + 1);
        let StdoutTarget::HeldFile { mut file, .. } = spec.stdout else {
            return Err("MySQL 必须使用已持有文件".into());
        };
        file.write_all(RAW)?;
        file.flush()?;
        *self.writer.borrow_mut() = Some(file.try_clone()?);
        if self.transport_error {
            return Err("明确的传输失败".into());
        }
        Ok(CommandOutput {
            status: status(self.code),
            stdout: Vec::new(),
            stderr: b"private-stdout".to_vec(),
        })
    }
}

#[test]
fn same_handle_snapshots_repeat_without_reopening_or_empty_fallback() {
    let fixture = Fixture::new();
    let path = fixture.context.observation_dir.join("held.stdout");
    let mut output = VerifiedStdout::create(&path).unwrap();
    write_held(&output, RAW);
    let first = output.snapshot().unwrap();
    let second = output.snapshot().unwrap();
    assert_eq!(first.raw, RAW);
    assert_eq!(first.raw, second.raw);
    assert_eq!(first.artifact, second.artifact);
    assert_eq!(first.artifact["bytes"], RAW.len());
    assert_eq!(
        first.artifact["sha256"],
        digest::file(&path).unwrap().sha256
    );
}

#[test]
fn registered_snapshot_drift_is_sticky_even_after_restoring_bytes() {
    let fixture = Fixture::new();
    let mut output =
        VerifiedStdout::create(&fixture.context.observation_dir.join("held.stdout")).unwrap();
    write_held(&output, RAW);
    output.snapshot().unwrap();
    write_held(&output, b"changed");
    assert_eq!(
        output.snapshot().unwrap_err().kind(),
        "StdoutIntegrityError"
    );
    write_held(&output, RAW);
    assert_eq!(
        output.snapshot().unwrap_err().kind(),
        "StdoutIntegrityError"
    );
}

#[test]
fn existing_or_relative_output_path_is_rejected() {
    let fixture = Fixture::new();
    let path = fixture.context.observation_dir.join("held.stdout");
    fs::write(&path, RAW).unwrap();
    assert!(VerifiedStdout::create(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), RAW);
    assert!(VerifiedStdout::create(Path::new("relative.stdout")).is_err());
}

#[test]
fn additional_hard_link_is_blocked_or_detected() {
    let fixture = Fixture::new();
    let path = fixture.context.observation_dir.join("held.stdout");
    let mut output = VerifiedStdout::create(&path).unwrap();
    write_held(&output, RAW);
    let link = fixture.context.observation_dir.join("extra.stdout");
    match fs::hard_link(&path, &link) {
        Ok(()) => assert_eq!(
            output.snapshot().unwrap_err().kind(),
            "StdoutIntegrityError"
        ),
        #[cfg(windows)]
        Err(error) if matches!(error.raw_os_error(), Some(5 | 32)) => {
            assert_eq!(output.snapshot().unwrap().raw, RAW);
        }
        Err(error) => panic!("无法建立同卷硬链接测试现场：{error}"),
    }
}

#[cfg(unix)]
#[test]
fn replacing_path_cannot_redirect_snapshot_to_another_file() {
    let fixture = Fixture::new();
    let path = fixture.context.observation_dir.join("held.stdout");
    let moved = path.with_extension("original");
    let mut output = VerifiedStdout::create(&path).unwrap();
    write_held(&output, RAW);
    fs::rename(&path, &moved).unwrap();
    fs::write(&path, RAW).unwrap();
    assert_eq!(
        output.snapshot().unwrap_err().kind(),
        "StdoutIntegrityError"
    );
    assert_eq!(fs::read(moved).unwrap(), RAW);
}

#[cfg(windows)]
#[test]
fn original_windows_handle_blocks_new_writers_and_rename() {
    let fixture = Fixture::new();
    let path = fixture.context.observation_dir.join("held.stdout");
    let mut output = VerifiedStdout::create(&path).unwrap();
    write_held(&output, RAW);
    assert!(fs::OpenOptions::new().write(true).open(&path).is_err());
    assert!(fs::rename(&path, path.with_extension("replaced")).is_err());
    assert_eq!(output.snapshot().unwrap().raw, RAW);
}

#[test]
fn system_transport_writes_more_than_capture_limit_through_held_file() {
    let fixture = Fixture::new();
    let path = fixture.context.observation_dir.join("large.stdout");
    let mut output = VerifiedStdout::create(&path).unwrap();
    let spec = fixture.system_spec(
        output.target().unwrap(),
        "import sys; sys.stdout.buffer.write(b'x' * (17 * 1024 * 1024))",
    );
    let result = SystemTransport.run(spec).unwrap();
    assert!(result.status.success());
    assert!(result.stdout.is_empty());
    let snapshot = output.snapshot().unwrap();
    assert_eq!(snapshot.raw.len(), 17 * 1024 * 1024);
    assert!(snapshot.raw.iter().all(|byte| *byte == b'x'));
    assert_eq!(snapshot.artifact, output.snapshot().unwrap().artifact);
}

#[test]
fn system_transport_retains_existing_capture_and_file_behavior() {
    let fixture = Fixture::new();
    let code = "import sys; sys.stdout.buffer.write(b'fixture')";
    let result = SystemTransport
        .run(fixture.system_spec(StdoutTarget::Capture, code))
        .unwrap();
    assert_eq!(result.stdout, b"fixture");
    let path = fixture.context.observation_dir.join("ordinary.stdout");
    let result = SystemTransport
        .run(fixture.system_spec(StdoutTarget::File(path.clone()), code))
        .unwrap();
    assert!(result.stdout.is_empty());
    assert_eq!(fs::read(path).unwrap(), b"fixture");
}

#[test]
fn nonzero_exit_keeps_raw_stdout_and_both_receipts_without_retry() {
    let fixture = Fixture::new();
    let fake = Fake::new(7, false);
    let mut evidence = Evidence::default();
    let port = Port {
        configuration: None,
        context: &fixture.context.storage_inputs(),
        transport: &fake,
        checkpoint: &|| Ok(()),
        directory: &fixture.context.observation_dir,
    };
    let error = evidence
        .mysql(&port, &fixture.database, "identity")
        .unwrap_err();
    assert!(error.downcast_ref::<super::VerificationError>().is_some());
    assert_eq!(fake.calls.get(), 1);
    let command = json_io::read(&fixture.files("command-")[0]).unwrap();
    let mysql = fixture
        .files("mysql-identity-")
        .into_iter()
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .unwrap();
    let mysql = json_io::read(&mysql).unwrap();
    assert_eq!(command["returncode"], 7);
    assert_eq!(mysql["returncode"], 7);
    assert_eq!(mysql["error_type"], "CommandFailed");
    assert_eq!(command["stdout_file"], mysql["stdout_file"]);
    assert!(!command.to_string().contains("private-stdout"));
    assert_eq!(
        fs::read(Path::new(mysql["stdout_file"]["path"].as_str().unwrap())).unwrap(),
        RAW
    );
    assert!(mysql.get("stdout_capture_error").is_none());
}

#[test]
fn transport_failure_keeps_successful_capture_without_fake_capture_error() {
    let fixture = Fixture::new();
    let fake = Fake::new(0, true);
    let mut evidence = Evidence::default();
    let port = Port {
        configuration: None,
        context: &fixture.context.storage_inputs(),
        transport: &fake,
        checkpoint: &|| Ok(()),
        directory: &fixture.context.observation_dir,
    };
    let error = evidence
        .mysql(&port, &fixture.database, "identity")
        .unwrap_err();
    assert_eq!(fake.calls.get(), 1);
    assert_eq!(error.source().unwrap().to_string(), "明确的传输失败");
    let command = json_io::read(&fixture.files("command-")[0]).unwrap();
    let mysql = fixture
        .files("mysql-identity-")
        .into_iter()
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .unwrap();
    let mysql = json_io::read(&mysql).unwrap();
    assert_eq!(command["error_type"], "TransportError");
    assert_eq!(mysql["error_type"], "TransportError");
    assert_eq!(command["stdout_file"], mysql["stdout_file"]);
    assert_eq!(mysql["stdout_file"]["bytes"], RAW.len());
    assert!(command.get("stdout_capture_error").is_none());
    assert!(mysql.get("stdout_capture_error").is_none());
}

#[test]
fn capture_failure_is_preserved_in_command_and_mysql_receipts() {
    let fixture = Fixture::new();
    let mut evidence = Evidence::default();
    let mut output =
        VerifiedStdout::create(&fixture.context.observation_dir.join("failed.stdout")).unwrap();
    write_held(&output, RAW);
    output.snapshot().unwrap();
    write_held(&output, b"changed-raw");
    output.snapshot().unwrap_err();
    let mut result: Result<CommandOutput> = Err(CommandFailure::transport("明确的传输失败".into()));
    let error = evidence
        .record_command(
            &fixture.context.observation_dir,
            &["mysql".into()],
            &[],
            &mut result,
            Some(&mut output),
        )
        .unwrap()
        .unwrap();
    assert_eq!(error.kind(), "StdoutIntegrityError");
    let snapshot = output.snapshot();
    let mysql = mysql_receipt(&fixture.database, "identity", &output, &result, &snapshot);
    evidence
        .write(&fixture.context.observation_dir, "failed.json", &mysql)
        .unwrap();
    let command = json_io::read(&fixture.files("command-")[0]).unwrap();
    assert!(command["stdout"].is_null());
    assert_eq!(command["error_type"], "TransportError");
    assert_eq!(mysql["error_type"], "TransportError");
    assert_eq!(command["stdout_capture_error"], "StdoutIntegrityError");
    assert_eq!(
        command["stdout_capture_error"],
        mysql["stdout_capture_error"]
    );
    assert_eq!(command["stdout_file"], mysql["stdout_file"]);
    assert!(mysql["stdout_file"].get("bytes").is_none());
    drop(output);
    assert_eq!(
        fs::read(fixture.context.observation_dir.join("failed.stdout")).unwrap(),
        b"changed-raw"
    );
}

#[test]
fn drift_between_outer_and_inner_capture_fails_before_response_is_used() {
    let fixture = Fixture::new();
    let fake = Fake::new(0, false);
    let checkpoints = Cell::new(0);
    let checkpoint = || {
        checkpoints.set(checkpoints.get() + 1);
        if checkpoints.get() == 2 {
            let mut writer = fake.writer.borrow_mut();
            let writer = writer.as_mut().unwrap();
            writer.seek(SeekFrom::Start(0))?;
            writer.set_len(0)?;
            writer.write_all(b"changed-after-command")?;
            writer.flush()?;
        }
        Ok(())
    };
    let port = Port {
        configuration: None,
        context: &fixture.context.storage_inputs(),
        transport: &fake,
        checkpoint: &checkpoint,
        directory: &fixture.context.observation_dir,
    };
    assert!(
        Evidence::default()
            .mysql(&port, &fixture.database, "identity")
            .is_err()
    );
    assert_eq!(fake.calls.get(), 1);
    let command = json_io::read(&fixture.files("command-")[0]).unwrap();
    let mysql = fixture
        .files("mysql-identity-")
        .into_iter()
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .unwrap();
    let mysql = json_io::read(&mysql).unwrap();
    assert!(command["stdout_file"].get("sha256").is_some());
    assert_eq!(mysql["stdout_capture_error"], "StdoutIntegrityError");
    assert_eq!(mysql["error_type"], "StdoutIntegrityError");
}
