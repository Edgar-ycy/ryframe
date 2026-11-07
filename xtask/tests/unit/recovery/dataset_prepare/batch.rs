use super::*;
use crate::recovery::clone::native::export::transport::{CommandOutput, CommandSpec};
use std::{cell::RefCell, fs, process::ExitStatus};

struct Fixture {
    temporary: tempfile::TempDir,
    plan: Value,
    dataset: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = crate::workspace::root_dir();
        let parent = root.join(".local-tests/dataset-batch-tests");
        fs::create_dir_all(&parent).unwrap();
        let temporary = tempfile::tempdir_in(parent).unwrap();
        let work = temporary.path();
        let program = work.join("mysql.exe");
        fs::write(&program, b"bound test tool").unwrap();
        let defaults = work.join("client.cnf");
        for name in ["ca.pem", "cert.pem", "key.pem"] {
            fs::write(work.join(name), b"bound TLS fixture").unwrap();
        }
        let bytes = format!(
            "[client]\nhost=127.0.0.1\nport=3306\nuser=fixture\npassword=private-secret-marker\nssl-mode=REQUIRED\nssl-ca={}\nssl-cert={}\nssl-key={}\n",
            work.join("ca.pem").display(),
            work.join("cert.pem").display(),
            work.join("key.pem").display()
        );
        fs::write(&defaults, &bytes).unwrap();
        let dataset = work.join("dataset");
        fs::create_dir(&dataset).unwrap();
        let plan = json!({"id":"restore-case","work_dir":work,
            "tools":{"mysql":{"path":program,"sha256":model::sha(b"bound test tool")}},
            "source":{"scope_id":"source-case","databases":[{"kind":"combined","database":"fixture","defaults_file":defaults,"defaults_sha256":model::sha(bytes.as_bytes())}]},
            "dataset":{"records":110,"api_validation_posts":2,"post_batch_rows":25}});
        Self {
            temporary,
            plan,
            dataset,
        }
    }
    fn run(&self, transport: &Fake) -> Result<Value> {
        run_with(
            &self.plan,
            &crate::workspace::root_dir(),
            &self.dataset,
            transport,
            || Ok(()),
        )
    }
}

struct Fake {
    calls: RefCell<Vec<CommandSpec>>,
    fail: usize,
    mutation: Option<std::path::PathBuf>,
    maximum: String,
    unexpected: bool,
    transport_error: bool,
    hide_directory: bool,
}
impl Fake {
    fn new() -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            fail: usize::MAX,
            mutation: None,
            maximum: "9007199254740992".into(),
            unexpected: false,
            transport_error: false,
            hide_directory: false,
        }
    }
}
impl Transport for Fake {
    fn run(&self, spec: CommandSpec) -> Result<CommandOutput> {
        let index = self.calls.borrow().len();
        assert!(spec.timeout <= Duration::from_secs(900));
        assert!(!spec.environment.contains_key("MYSQL_PWD"));
        assert!(spec.args[0].starts_with("--defaults-file="));
        let work = spec.work_dir.clone();
        self.calls.borrow_mut().push(spec);
        if index == 1 && self.hide_directory {
            fs::rename(&work, work.with_extension("preserved")).unwrap();
        }
        if index == 1 && self.transport_error {
            return Err(std::io::Error::from_raw_os_error(8).into());
        }
        if index == 1
            && let Some(path) = &self.mutation
        {
            fs::write(path, b"changed").unwrap();
        }
        #[cfg(windows)]
        let status = {
            use std::os::windows::process::ExitStatusExt;
            ExitStatus::from_raw(u32::from(index == self.fail))
        };
        #[cfg(unix)]
        let status = {
            use std::os::unix::process::ExitStatusExt;
            ExitStatus::from_raw(i32::from(index == self.fail) << 8)
        };
        Ok(CommandOutput {
            status,
            stdout: if index == 0 {
                self.maximum.as_bytes().to_vec()
            } else if self.unexpected {
                b"unexpected".to_vec()
            } else {
                Vec::new()
            },
            stderr: if index == self.fail {
                b"ERROR 1045 (28000): private-secret-marker".to_vec()
            } else {
                Vec::new()
            },
        })
    }
    fn s3(&self, _: crate::recovery::clone::native::s3::Request) -> Result<Value> {
        panic!("岗位批次不得访问对象存储")
    }
}

#[test]
fn native_batch_preserves_deterministic_rows_transactions_and_large_ids() {
    let fixture = Fixture::new();
    let transport = Fake::new();
    let result = fixture.run(&transport).unwrap();
    assert_eq!(result["result"]["first_id"], "9007199254740993");
    assert_eq!(result["result"]["last_id"], "9007199254741080");
    assert_eq!(result["result"]["batches"], 4);
    assert_eq!(result["samples"].as_array().unwrap().len(), 22);
    let mut tuples = Vec::new();
    let calls = transport.calls.borrow();
    for call in &calls[1..] {
        let sql = std::str::from_utf8(call.stdin.as_ref().unwrap()).unwrap();
        assert_eq!(sql.matches("START TRANSACTION;").count(), 1);
        assert!(sql.ends_with("COMMIT;\n"));
        assert!(!sql.contains("恢复样本"));
        let rows = sql
            .lines()
            .filter(|line| line.starts_with('('))
            .collect::<Vec<_>>();
        assert!(rows.len() <= 25);
        tuples.extend(rows.into_iter().map(str::to_owned));
    }
    let rows = input::read(&fixture.plan, &fixture.dataset.join("post-batch.ndjson")).unwrap();
    assert_eq!(rows.len(), 88);
    for (position, (row, tuple)) in rows.iter().zip(&tuples).enumerate() {
        assert!(tuple.starts_with(&format!("({},", 9007199254740993i64 + position as i64)));
        for field in ["tenant_id", "name", "code"] {
            assert!(tuple.contains(&format!(
                "CONVERT(0x{} USING utf8mb4)",
                hex::encode(row[field].as_str().unwrap().as_bytes())
            )));
        }
    }
    assert_eq!(tuples.len(), 88);
    let input = fixture.dataset.join("post-batch.ndjson");
    let mut lines = fs::read_to_string(&input)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    lines[0] = lines[1].clone();
    fs::write(&input, lines.join("\n")).unwrap();
    assert!(input::read(&fixture.plan, &input).is_err());
}

#[test]
fn partial_failure_unknown_stdout_and_drift_never_replay_or_leak_secrets() {
    for mode in [
        "exit",
        "stdout",
        "input",
        "tool",
        "tls",
        "transport",
        "receipt",
    ] {
        let fixture = Fixture::new();
        let mut fake = Fake::new();
        if mode == "exit" {
            fake.fail = 2;
        }
        if mode == "stdout" {
            fake.unexpected = true;
        }
        if mode == "transport" {
            fake.transport_error = true;
        }
        if mode == "receipt" {
            fake.fail = 1;
            fake.hide_directory = true;
        }
        if mode == "input" {
            fake.mutation = Some(fixture.dataset.join("post-batch.ndjson"));
        }
        if mode == "tool" {
            fake.mutation = Some(fixture.temporary.path().join("mysql.exe"));
        }
        if mode == "tls" {
            fake.mutation = Some(fixture.temporary.path().join("cert.pem"));
        }
        let error = fixture.run(&fake).unwrap_err().to_string();
        assert!(!error.contains("private-secret-marker"));
        assert!(error.contains("unknown=true"));
        if mode == "exit" || mode == "receipt" {
            for identity in ["阶段=insert", "exit=1", "1045", "28000", "认证拒绝"] {
                assert!(error.contains(identity), "{error}");
            }
        }
        assert_eq!(
            fake.calls.borrow().len(),
            if mode == "exit" { 3 } else { 2 }
        );
        if mode == "receipt" {
            assert!(error.contains("不能确认失败收据发布"));
            assert!(
                fixture
                    .dataset
                    .with_extension("preserved")
                    .join("post-batch.ndjson")
                    .is_file()
            );
            continue;
        }
        let failures = fs::read_dir(&fixture.dataset)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect::<Vec<_>>();
        assert!(!failures.is_empty());
        let mut identified = false;
        for path in failures {
            if mode == "exit" || mode == "transport" {
                assert!(error.contains(path.to_str().unwrap()), "{error}");
            }
            let bytes = fs::read_to_string(path).unwrap();
            assert!(!bytes.contains("private-secret-marker"));
            let value: Value = serde_json::from_str(&bytes).unwrap();
            assert_eq!(value["possible_write"], true);
            assert_eq!(
                value["completed_batches"],
                if mode == "exit" { 1 } else { 0 }
            );
            if value["stage"] == "insert" {
                assert_eq!(value["detail"]["mysql_error"], 1045);
                assert_eq!(value["detail"]["sqlstate"], "28000");
                assert_eq!(value["detail"]["exit_code"], 1);
                identified = true;
            }
            if value["stage"] == "process" {
                assert_eq!(value["detail"]["system_code"], 8);
            }
        }
        assert_eq!(identified, mode == "exit");
        assert!(fixture.dataset.join("post-batch.ndjson").is_file());
    }
}

#[test]
fn exhausted_id_range_or_implicit_login_fails_before_any_insert() {
    for mode in ["range", "login", "input"] {
        let fixture = Fixture::new();
        let mut fake = Fake::new();
        fake.maximum = i64::MAX.to_string();
        if mode == "login" {
            fs::write(
                fixture.temporary.path().join("unused-login.cnf"),
                b"do not read",
            )
            .unwrap();
        }
        let result = if mode == "input" {
            run_with(
                &fixture.plan,
                &crate::workspace::root_dir(),
                &fixture.dataset,
                &fake,
                || {
                    let input = fixture.dataset.join("post-batch.ndjson");
                    if input.exists() {
                        fs::write(input, b"changed before MAX")?;
                    }
                    Ok(())
                },
            )
        } else {
            fixture.run(&fake)
        };
        assert!(result.is_err());
        assert_eq!(
            fake.calls.borrow().len(),
            if mode == "range" { 1 } else { 0 }
        );
    }
}
