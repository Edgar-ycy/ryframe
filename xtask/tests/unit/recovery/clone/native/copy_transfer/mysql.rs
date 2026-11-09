use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    process::ExitStatus,
};

use super::super::super::{export::transport::CommandOutput, s3};
use super::*;

const SESSION: &str = "session-0123456789abcdef0123456789abcdef";
const SQL: &[u8] =
    b"-- ryframe-reference-data-v1\r\nINSERT INTO `a_business` (`id`) VALUES (12);\r\n";

struct Fixture {
    _temp: tempfile::TempDir,
    backend: PathBuf,
    copy: PathBuf,
    export: PathBuf,
    defaults: PathBuf,
    tool: PathBuf,
    environment: BTreeMap<String, String>,
    request: Value,
    item: Value,
}

impl Fixture {
    fn new() -> Self {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let temp = tempfile::tempdir_in(local).unwrap();
        let backend = crate::recovery::fixture_control::portable_path(
            &fs::canonicalize(temp.path()).unwrap(),
        );
        let local = backend.join(".local-tests");
        let copy = local.join("copy");
        let export = local.join("export");
        let private = local.join("private");
        for dir in [&copy, &export, &private] {
            fs::create_dir_all(dir).unwrap();
        }
        let defaults = private.join("target.cnf");
        let tool = local.join("mysql.exe");
        fs::write(&defaults, "[client]\nhost=127.0.0.1\nport=3307\nuser=fixture\npassword=fixture-private\nssl-mode=DISABLED\n").unwrap();
        fs::write(&tool, b"fixture: mock never launches a program").unwrap();
        let request = json!({
            "target":{"databases":[{"key":"shared-control","database":"target_control",
                "defaults_file":defaults,"defaults_sha256":digest::file(&defaults).unwrap().sha256}]},
            "tools":{"mysql":{"path":tool,"sha256":digest::file(&tool).unwrap().sha256}},
        });
        let environment = BTreeMap::from([
            (
                "TEMP".into(),
                common::path_text(&private, "测试 TEMP").unwrap(),
            ),
            ("mysql_pwd".into(), "ignored-password".into()),
            ("mysql_test_login_file".into(), "ignored-login-path".into()),
        ]);
        let mut fixture = Self {
            _temp: temp,
            backend,
            copy,
            export,
            defaults,
            tool,
            environment,
            request,
            item: json!({
                "key":"shared-control","source_database":"source_control",
                "target_database":"target_control","tables":{"z_business":0,"a_business":1},
                "preserve_target_ownership":true,
            }),
        };
        fixture.bind(SQL);
        fixture
    }

    fn bind(&mut self, sql: &[u8]) {
        fs::write(self.export.join("control.sql"), sql).unwrap();
        let expected = digest::file(&self.export.join("control.sql")).unwrap();
        self.item["artifact"] = json!({
            "file":"control.sql", "bytes":expected.bytes,"sha256":expected.sha256
        });
    }

    fn write(&self, mock: &Mock<'_>, checkpoint: &dyn Fn() -> Result<()>) -> Result<()> {
        write(
            &StorageInputs {
                backend: &self.backend,
                request: &self.request,
                environment: &self.environment,
                observation_dir: &self.copy,
            },
            &Write {
                copy_root: &self.copy,
                session_id: SESSION,
                export_root: &self.export,
                item: &self.item,
            },
            mock,
            checkpoint,
        )
    }

    fn receipt(&self) -> Value {
        let paths: Vec<_> = fs::read_dir(&self.copy)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(paths.len(), 1);
        assert!(
            paths[0]
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("command-{SESSION}-"))
        );
        document::read(&paths[0]).unwrap()
    }
}

struct Mock<'a> {
    fixture: &'a Fixture,
    observed: RefCell<Vec<CommandSpec>>,
    exit: Option<i32>,
}

impl<'a> Mock<'a> {
    fn new(fixture: &'a Fixture, exit: Option<i32>) -> Self {
        Self {
            fixture,
            observed: RefCell::new(Vec::new()),
            exit,
        }
    }
}

impl Transport for Mock<'_> {
    fn run(&self, _: CommandSpec) -> Result<CommandOutput> {
        panic!("敏感命令禁止回退到具名捕获")
    }

    fn run_private(&self, spec: CommandSpec) -> Result<CommandOutput> {
        let before = self.fixture.receipt();
        assert_eq!(before["status"], "pending");
        assert!(before["returncode"].is_null());
        self.observed.borrow_mut().push(spec);
        let code = self.exit.ok_or("模拟错误：fixture-private 和原始 SQL")?;
        Ok(CommandOutput {
            status: exit_status(code),
            stdout: b"fixture-private stdout".to_vec(),
            stderr: SQL.to_vec(),
        })
    }

    fn s3(&self, _: s3::Request) -> Result<Value> {
        panic!("数据库复制不访问对象存储")
    }
}

fn exit_status(code: i32) -> ExitStatus {
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        ExitStatus::from_raw(code as u32)
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(code << 8)
    }
}

#[test]
fn one_private_invocation_uses_the_exact_validated_sql_bytes_and_registered_database() {
    let fixture = Fixture::new();
    let mock = Mock::new(&fixture, Some(0));
    let checkpoints = Cell::new(0);
    fixture
        .write(&mock, &|| {
            checkpoints.set(checkpoints.get() + 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(checkpoints.get(), 4);
    let commands = mock.observed.borrow();
    assert_eq!(commands.len(), 1);
    let command = &commands[0];
    assert_eq!(command.program, fixture.tool);
    assert_eq!(command.cwd, fixture.backend);
    assert_eq!(
        command.args,
        [
            format!("--defaults-file={}", fixture.defaults.display()),
            "--database=target_control".into(),
            "--batch".into(),
            "--raw".into(),
            "--skip-column-names".into(),
            "--binary-mode".into(),
            "--default-character-set=utf8mb4".into(),
        ]
    );
    assert_eq!(command.timeout, Duration::from_secs(1800));
    assert!(
        command
            .environment
            .keys()
            .all(|name| !name.eq_ignore_ascii_case("MYSQL_PWD"))
    );
    assert_eq!(
        command
            .environment
            .keys()
            .filter(|name| name.eq_ignore_ascii_case("MYSQL_TEST_LOGIN_FILE"))
            .count(),
        1
    );
    assert_eq!(
        Path::new(&command.environment["MYSQL_TEST_LOGIN_FILE"]),
        command.work_dir.join("unused-login.cnf")
    );
    let mut expected = concat!(
        "SET SESSION FOREIGN_KEY_CHECKS=0; SET SESSION time_zone='+00:00'; ",
        "SET SESSION SQL_MODE='NO_AUTO_VALUE_ON_ZERO'; START TRANSACTION;\n",
        "DELETE FROM `a_business`;\nDELETE FROM `z_business`;\n",
    )
    .as_bytes()
    .to_vec();
    expected.extend_from_slice(SQL);
    expected.extend_from_slice(b"\nCOMMIT;\n");
    assert_eq!(command.stdin.as_ref(), Some(&expected));
    let receipt = fixture.receipt();
    assert_eq!(receipt["status"], "exited_zero");
    assert_eq!(receipt["returncode"], 0);
    let evidence = receipt.to_string();
    assert!(!evidence.contains("fixture-private"));
    assert!(!evidence.contains("INSERT INTO"));
}

#[test]
fn artifact_and_target_validation_fail_before_invocation_or_command_receipt() {
    let mut fixture = Fixture::new();
    let sql = fixture.export.join("control.sql");
    let original = fixture.item.clone();
    let drift = String::from_utf8(SQL.to_vec())
        .unwrap()
        .replace("(12)", "(13)");
    fs::write(&sql, drift.as_bytes()).unwrap();
    assert!(
        fixture
            .write(&Mock::new(&fixture, Some(0)), &|| Ok(()))
            .is_err()
    );
    fixture.bind(drift.replace("a_business", "unauthorized").as_bytes());
    assert!(
        fixture
            .write(&Mock::new(&fixture, Some(0)), &|| Ok(()))
            .is_err()
    );
    fixture.bind(SQL);
    fixture.item["artifact"]["sha256"] = json!("0".repeat(64));
    assert!(
        fixture
            .write(&Mock::new(&fixture, Some(0)), &|| Ok(()))
            .is_err()
    );
    fixture.item = original;
    fixture.item["tables"]["ryframe_resource_ownership"] = json!(0);
    assert!(
        fixture
            .write(&Mock::new(&fixture, Some(0)), &|| Ok(()))
            .is_err()
    );
    fixture.item["tables"]
        .as_object_mut()
        .unwrap()
        .remove("ryframe_resource_ownership");
    fixture.item["target_database"] = json!("other_target");
    assert!(
        fixture
            .write(&Mock::new(&fixture, Some(0)), &|| Ok(()))
            .is_err()
    );
    fixture.item["target_database"] = json!("target_control");
    fs::write(&fixture.defaults, b"changed private configuration").unwrap();
    let mock = Mock::new(&fixture, Some(0));
    assert!(fixture.write(&mock, &|| Ok(())).is_err());
    assert!(mock.observed.borrow().is_empty());
    assert_eq!(fs::read_dir(&fixture.copy).unwrap().count(), 0);
}

#[test]
fn failures_keep_the_raw_exit_status_or_unknown_without_retrying() {
    for (exit, failed_checkpoint, error) in [
        (Some(3), false, "CommandFailed"),
        (None, false, "CommandError"),
        (Some(0), true, "CheckpointFailed"),
    ] {
        let fixture = Fixture::new();
        let mock = Mock::new(&fixture, exit);
        let check = || {
            if failed_checkpoint && !mock.observed.borrow().is_empty() {
                Err("模拟变化：fixture-private".into())
            } else {
                Ok(())
            }
        };
        let result = fixture.write(&mock, &check).unwrap_err();
        assert_eq!(mock.observed.borrow().len(), 1);
        assert!(!result.to_string().contains("fixture-private"));
        let receipt = fixture.receipt();
        assert_eq!(receipt["returncode"], json!(exit));
        assert_eq!(receipt["status"], "unknown");
        assert_eq!(receipt["error_type"], error);
        assert_eq!(receipt["remote_write_may_have_occurred"], true);
        assert_eq!(receipt["automatic_retry_permitted"], false);
        assert!(!receipt.to_string().contains("fixture-private"));
        assert!(!receipt.to_string().contains("INSERT INTO"));
    }
}

#[test]
fn drift_after_command_receipt_refuses_to_start_mysql() {
    let fixture = Fixture::new();
    let mock = Mock::new(&fixture, Some(0));
    let calls = Cell::new(0);
    let checkpoint = || {
        calls.set(calls.get() + 1);
        if calls.get() == 3 {
            Err("调用前绑定漂移".into())
        } else {
            Ok(())
        }
    };
    assert!(fixture.write(&mock, &checkpoint).is_err());
    assert!(mock.observed.borrow().is_empty());
    let receipt = fixture.receipt();
    assert_eq!(receipt["status"], "not_started");
    assert_eq!(receipt["remote_write_may_have_occurred"], false);
    assert!(receipt["returncode"].is_null());
}
