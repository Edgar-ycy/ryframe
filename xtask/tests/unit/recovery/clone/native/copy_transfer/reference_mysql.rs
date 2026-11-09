use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    process::ExitStatus,
};

use super::*;
use crate::recovery::clone::native::{digest, s3};

const SQL: &[u8] =
    b"-- ryframe-reference-data-v1\r\nINSERT INTO `a_business` (`id`) VALUES (12);\r\n";

struct Fixture {
    _temp: tempfile::TempDir,
    backend: PathBuf,
    work: PathBuf,
    backup: PathBuf,
    sql: PathBuf,
    mysql: Value,
    request: Value,
    environment: BTreeMap<String, String>,
    task: Value,
    artifact: Value,
}

impl Fixture {
    fn new() -> Self {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("restore-mysql-")
            .tempdir_in(local)
            .unwrap();
        let backend =
            crate::recovery::fixture_control::portable_path(&temp.path().canonicalize().unwrap());
        let local = backend.join(".local-tests");
        let work = local.join("work");
        let backup = local.join("backup");
        let private = local.join("private");
        for dir in [&work, &backup.join("databases"), &private] {
            fs::create_dir_all(dir).unwrap();
        }
        let defaults = private.join("target.cnf");
        fs::write(&defaults, "[client]\nhost=127.0.0.1\nport=3307\nuser=fixture\npassword=fixture-private\nssl-mode=REQUIRED\nssl-ca=fixture-ca.pem\n").unwrap();
        let database = json!({"key":"shared-control","kind":"combined","mode":"shared",
            "database":"target_control","server_uuid":"registered-fixture",
            "defaults_file":defaults,"defaults_sha256":digest::file(&defaults).unwrap().sha256});
        let make_tool = |name: &str| {
            let path = local.join(name);
            fs::write(&path, name.as_bytes()).unwrap();
            json!({"path":path,"sha256":digest::file(&path).unwrap().sha256})
        };
        let request = json!({
            "target":{"databases":[database]},"tools":{"mysql":make_tool("arm-mysql.exe")},
        });
        let mysql = make_tool("reference-mysql.exe");
        let environment = BTreeMap::from([
            ("TEMP".into(), private.to_string_lossy().into_owned()),
            ("TMP".into(), private.to_string_lossy().into_owned()),
            ("mysql_pwd".into(), "ignored-credential".into()),
            ("mysql_test_login_file".into(), "ignored-login".into()),
        ]);
        let relative = "databases/shared-control.sql";
        let sql = backup.join(relative);
        fs::write(&sql, SQL).unwrap();
        let digest = digest::file(&sql).unwrap();
        let artifact = json!({"path":sql,"bytes":digest.bytes,"sha256":digest.sha256});
        let task = json!({"kind":"database","target":database,"relative":relative,
            "tables":["z_business","a_business","seaql_tenant_data_migrations"]});
        Self {
            _temp: temp,
            backend,
            work,
            backup,
            sql,
            mysql,
            request,
            environment,
            task,
            artifact,
        }
    }

    fn write(&self, mock: &Mock<'_>, checkpoint: &dyn Fn() -> Result<()>) -> Result<()> {
        write(
            &StorageInputs {
                backend: &self.backend,
                request: &self.request,
                environment: &self.environment,
                observation_dir: &self.work,
            },
            &Write {
                backup_root: &self.backup,
                work: &self.work,
                task: &self.task,
                artifact: &self.artifact,
                mysql: &self.mysql,
            },
            mock,
            checkpoint,
        )
    }

    fn evidence(&self, suffix: &str) -> (PathBuf, Value) {
        let path = fs::read_dir(&self.work)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|file| file.to_string_lossy().ends_with(suffix))
            .unwrap();
        let value = document::read(&path).unwrap();
        (path, value)
    }
}

struct Mock<'a> {
    fixture: &'a Fixture,
    calls: RefCell<Vec<CommandSpec>>,
    exit: Option<i32>,
    drift_intent: bool,
}

impl<'a> Mock<'a> {
    fn new(fixture: &'a Fixture, exit: Option<i32>) -> Self {
        Self {
            fixture,
            calls: RefCell::new(Vec::new()),
            exit,
            drift_intent: false,
        }
    }
}

impl Transport for Mock<'_> {
    fn run(&self, _: CommandSpec) -> Result<CommandOutput> {
        panic!("参考恢复不得用普通捕获发送私有 SQL")
    }

    fn run_private(&self, command: CommandSpec) -> Result<CommandOutput> {
        let (intent, _) = self.fixture.evidence(".intent.json");
        let (_, pending) = self.fixture.evidence(".diagnostic.json");
        assert_eq!(pending["intent"], common::binding(&intent)?);
        assert_eq!(pending["status"], "pending");
        self.calls.borrow_mut().push(command);
        if self.drift_intent {
            fs::write(intent, br#"{"changed":true}"#)?;
        }
        let code = self.exit.ok_or("模拟私有 stderr")?;
        Ok(CommandOutput {
            status: exit_status(code),
            stdout: b"fixture-private".to_vec(),
            stderr: SQL.to_vec(),
        })
    }

    fn s3(&self, _: s3::Request) -> Result<Value> {
        panic!("SQL 恢复不得访问对象存储")
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
fn restore_submits_original_sql_in_inventory_order_using_reference_tool() {
    let fixture = Fixture::new();
    let mock = Mock::new(&fixture, Some(0));
    fixture.write(&mock, &|| Ok(())).unwrap();
    let calls = mock.calls.borrow();
    assert_eq!(calls.len(), 1);
    let command = &calls[0];
    assert_eq!(
        command.program,
        Path::new(fixture.mysql["path"].as_str().unwrap())
    );
    assert_eq!(command.cwd, fixture.backend);
    assert_eq!(command.args[1], "--database=target_control");
    assert_eq!(command.timeout, std::time::Duration::from_secs(1800));
    assert!(
        command
            .environment
            .keys()
            .all(|key| !key.eq_ignore_ascii_case("MYSQL_PWD"))
    );
    let mut expected = concat!(
        "SET SESSION FOREIGN_KEY_CHECKS=0; SET SESSION time_zone='+00:00'; ",
        "SET SESSION SQL_MODE='NO_AUTO_VALUE_ON_ZERO'; START TRANSACTION;\n",
        "DELETE FROM `z_business`;\nDELETE FROM `a_business`;\n",
        "DELETE FROM `seaql_tenant_data_migrations`;\n",
    )
    .as_bytes()
    .to_vec();
    expected.extend_from_slice(SQL);
    expected.extend_from_slice(b"\nCOMMIT;\n");
    assert_eq!(command.stdin.as_ref(), Some(&expected));
    let (path, intent) = fixture.evidence(".intent.json");
    let (_, diagnostic) = fixture.evidence(".diagnostic.json");
    assert_eq!(diagnostic["intent"], common::binding(&path).unwrap());
    assert_eq!(diagnostic["status"], "exited_zero");
    assert_eq!(diagnostic["returncode"], 0);
    let evidence = format!("{intent}{diagnostic}");
    assert!(!evidence.contains("fixture-private"));
    assert!(!evidence.contains("INSERT INTO"));
}

#[test]
fn restore_refuses_same_length_drift_foreign_paths_and_reserved_targets_before_invocation() {
    let mut fixture = Fixture::new();
    let original = fixture.artifact.clone();
    let another = fixture.backup.join("other.sql");
    fs::copy(&fixture.sql, &another).unwrap();
    fixture.artifact["path"] = json!(another);
    let mock = Mock::new(&fixture, Some(0));
    assert!(fixture.write(&mock, &|| Ok(())).is_err());
    assert!(mock.calls.borrow().is_empty());
    fixture.artifact = original;
    for (field, value) in [
        ("server_uuid", json!("changed")),
        ("mode", json!("dedicated")),
    ] {
        let old = fixture.task["target"][field].clone();
        fixture.task["target"][field] = value;
        assert!(
            fixture
                .write(&Mock::new(&fixture, Some(0)), &|| Ok(()))
                .is_err()
        );
        fixture.task["target"][field] = old;
    }
    let old = fixture.task["tables"].clone();
    for reserved in domain::EXCLUDED_TABLES {
        fixture.task["tables"] = json!([reserved]);
        assert!(
            fixture
                .write(&Mock::new(&fixture, Some(0)), &|| Ok(()))
                .is_err()
        );
    }
    fixture.task["tables"] = old;
    let changed = std::str::from_utf8(SQL).unwrap().replace("(12)", "(13)");
    fs::write(&fixture.sql, changed).unwrap();
    let mock = Mock::new(&fixture, Some(0));
    assert!(fixture.write(&mock, &|| Ok(())).is_err());
    assert!(mock.calls.borrow().is_empty());
    assert_eq!(fs::read_dir(&fixture.work).unwrap().count(), 0);
}

#[test]
fn restore_command_keeps_status_only_and_does_not_retry_on_unknown_or_guard_drift() {
    for (exit, stop_at, expected, attempts) in [
        (Some(17), 0, "unknown", 1),
        (None, 0, "unknown", 1),
        (Some(0), 3, "not_started", 0),
        (Some(0), 4, "unknown", 1),
    ] {
        let fixture = Fixture::new();
        let mock = Mock::new(&fixture, exit);
        let checkpoints = Cell::new(0);
        assert!(
            fixture
                .write(&mock, &|| {
                    let number = checkpoints.get() + 1;
                    checkpoints.set(number);
                    if number == stop_at {
                        Err("模拟私有检查点".into())
                    } else {
                        Ok(())
                    }
                })
                .is_err()
        );
        assert_eq!(mock.calls.borrow().len(), attempts);
        let (_, diagnostic) = fixture.evidence(".diagnostic.json");
        assert_eq!(diagnostic["status"], expected);
        assert_eq!(
            diagnostic["returncode"],
            json!(if attempts == 0 { None } else { exit })
        );
        assert_eq!(diagnostic["remote_write_may_have_occurred"], attempts > 0);
        assert_eq!(diagnostic["automatic_retry_permitted"], false);
        assert!(!diagnostic.to_string().contains("模拟私有"));
    }
}

#[test]
fn changed_original_intent_prevents_replacing_the_pending_diagnostic() {
    let fixture = Fixture::new();
    let mut mock = Mock::new(&fixture, Some(0));
    mock.drift_intent = true;
    assert!(fixture.write(&mock, &|| Ok(())).is_err());
    assert_eq!(mock.calls.borrow().len(), 1);
    let (_, diagnostic) = fixture.evidence(".diagnostic.json");
    assert_eq!(diagnostic["status"], "pending");
    let (_, changed) = fixture.evidence(".intent.json");
    assert_eq!(changed, json!({"changed":true}));
}
