use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    io::Write,
    process::ExitStatus,
};

use super::*;
use crate::recovery::clone::native::{
    export::transport::CommandOutput, s3::Request, target_verify::input::request::KEYS,
};

const SOURCE: &str = "fixture-source";
const TARGET: &str = "fixture-target";
const UUID: &str = "67cd8d4a-fe90-11ef-98fd-5847ca786499";
const ID: i64 = 9012345678901234567;
const AT: &str = "2026-11-09 10:00:00.000001";

pub(in crate::recovery::clone::native::post_process) fn before() -> Value {
    json!({"database_now":AT,"row":{
        "id":ID,"tenant_id":"system","name":"调度夹具","handler_key":"system.cleanup",
        "cron_expression":"0 0 1 * * *","timezone":"UTC","enabled":1,"misfire_policy":"fire_once",
        "concurrency_policy":"forbid","max_runtime_seconds":900,"next_run_at":"2026-11-10 01:00:00",
        "last_run_at":null,"version":5,"del_flag":"0","created_at":"2026-11-08 00:00:00","updated_at":"2026-11-08 00:00:00"
    }})
}

pub(in crate::recovery::clone::native::post_process) fn action(
    execution: &Path,
    observed: &Value,
) -> Value {
    let (_, row) = observation(observed, &columns(execution).unwrap()).unwrap();
    json!({"action":"disable_schedule_via_api","tenant_id":"system","schedule_id":ID.to_string(),
        "handler_key":"system.cleanup","expected_version":5,"source_row_sha256":schedule::row_sha256(&row).unwrap()})
}

#[test]
fn full_row_hash_versions_and_database_clock_preserve_the_original_inspection_shape() {
    let backend = crate::workspace::root_dir();
    let initial = before();
    let action = action(&backend, &initial);
    validate_before(&backend, &action, &initial).unwrap();
    let mut after = initial.clone();
    after["row"]["enabled"] = json!(0);
    after["row"]["next_run_at"] = Value::Null;
    after["row"]["version"] = json!(6);
    after["row"]["updated_at"] = json!(AT);
    let result = inspect_disabled(&backend, &action, &initial, &after).unwrap();
    assert_eq!(result["status"], "disabled_row_verified");
    assert_eq!(result["before_sha256"], action["source_row_sha256"]);
    assert_ne!(result["before_sha256"], result["observed_sha256"]);
    assert_eq!(result["schedule_id"], action["schedule_id"]);
    for key in [
        "automatic_retry_allowed",
        "target_ready",
        "api_execution_proven",
    ] {
        assert_eq!(result[key], false);
    }
    assert_eq!(result["worker_must_remain_stopped"], true);
    let mut normalized = initial.clone();
    normalized["row"]["created_at"] = json!("2026-11-08 00:00:00.000000");
    assert_eq!(
        inspect_disabled(&backend, &action, &initial, &normalized).unwrap()["status"],
        "unchanged_before"
    );
    after["row"]["name"] = json!("变化");
    assert_eq!(
        inspect_disabled(&backend, &action, &initial, &after).unwrap()["status"],
        "needs_reconciliation"
    );
    after["database_now"] = json!("2026-11-09 09:59:59");
    assert!(inspect_disabled(&backend, &action, &initial, &after).is_err());
}

#[test]
fn action_and_api_projection_reject_partial_rows_wrong_types_and_unbound_actions() {
    let backend = crate::workspace::root_dir();
    let initial = before();
    let action = action(&backend, &initial);
    let mut pending = action.clone();
    pending["operations"] = json!(["pending"]);
    assert_eq!(action_input(&pending).unwrap(), action);
    pending["schedule_id"] = json!("1;SELECT 2");
    assert!(action_input(&pending).is_err());
    let mut api = json!({"id":ID.to_string(),"handler_key":"system.cleanup","version":6,"enabled":false,"extra":true});
    validate_api_row(&api, &action, false, 6).unwrap();
    api["version"] = json!(true);
    assert!(validate_api_row(&api, &action, false, 6).is_err());
    let mut wrong = initial.clone();
    wrong["row"]["enabled"] = json!(true);
    assert!(validate_before(&backend, &action, &wrong).is_err());
    wrong = initial.clone();
    wrong["row"].as_object_mut().unwrap().remove("timezone");
    assert!(validate_before(&backend, &action, &wrong).is_err());
    wrong = initial;
    wrong["database_now"] = json!("2026-11-09 10:00:00.000001Z");
    assert!(validate_before(&backend, &action, &wrong).is_err());
}

struct Fixture {
    _temp: tempfile::TempDir,
    backend: PathBuf,
    output: PathBuf,
    source: Value,
    target: Value,
    environment: BTreeMap<String, String>,
    source_environment: BTreeMap<String, String>,
}

impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let local = backend.join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let temp = tempfile::tempdir_in(&local).unwrap();
        let root = temp.path();
        let config = root.join("config");
        let output = root.join("evidence");
        let private = root.join("private");
        for directory in [&config, &output, &private] {
            fs::create_dir(directory).unwrap();
        }
        fs::write(config.join("app.test.toml"),
            b"[object_storage]\nendpoint='http://127.0.0.1:39000'\nregion='us-east-1'\naccess_key='private-fixture-key'\nsecret_key='private-fixture-secret'\n").unwrap();
        let defaults = private.join("mysql.cnf");
        fs::write(&defaults,b"[client]\nhost=127.0.0.1\nport=3307\nuser=fixture\npassword=private-fixture-password\nssl-mode=DISABLED\n").unwrap();
        let mysql = root.join("mysql.exe");
        fs::write(&mysql, b"fixed mock").unwrap();
        let environment = BTreeMap::from([
            ("APP_ENV".into(), "test".into()),
            ("TEMP".into(), private.to_string_lossy().into_owned()),
            ("TMP".into(), private.to_string_lossy().into_owned()),
        ]);
        let request = |scope: &str, prefix: &str| {
            let databases = KEYS.map(|(key,kind,mode)| json!({
                "key":key,"kind":kind,"mode":mode,"database":format!("{prefix}_{}",key.replace('-',"_")),
                "server_uuid":UUID,"defaults_file":defaults,"defaults_sha256":digest::file(&defaults).unwrap().sha256,
            }));
            json!({"target":{"scope_id":scope,"databases":databases,
                "s3":{"endpoint":"http://127.0.0.1:39000","region":"us-east-1","access_key":"private-fixture-key","secret_key":"private-fixture-secret"}},
                "config_dir":config,"tools":{"mysql":{"path":mysql,"sha256":digest::file(&mysql).unwrap().sha256}}})
        };
        let mut target = request(TARGET, "target");
        let (path, files) =
            request_configuration::fresh_files(&backend, &environment, &target).unwrap();
        target["configuration_sha256"] =
            json!(request_configuration::configuration_identity(&path, &files).unwrap());
        let mut source = request(SOURCE, "source");
        source["target"]["s3"] = json!({"configuration":app_configuration::configuration_value(&backend,&config,"test").unwrap()});
        Self {
            _temp: temp,
            backend,
            output,
            source,
            target,
            environment,
            source_environment: BTreeMap::from([("APP_ENV".into(), "test".into())]),
        }
    }

    fn storage(&self, side: CloneSide) -> StorageInputs<'_> {
        StorageInputs {
            backend: &self.backend,
            request: if side == CloneSide::Source {
                &self.source
            } else {
                &self.target
            },
            environment: if side == CloneSide::Source {
                &self.source_environment
            } else {
                &self.environment
            },
            observation_dir: &self.output,
        }
    }
}

#[derive(Default)]
struct Fake {
    calls: RefCell<Vec<(String, String)>>,
    empty_source_identity: Cell<bool>,
    schedule_count: Cell<Option<&'static str>>,
}

impl Transport for Fake {
    fn run(&self, spec: CommandSpec) -> Result<CommandOutput> {
        assert!(spec.stdin.is_none());
        assert_eq!(spec.args[spec.args.len() - 2], "--execute");
        let db = spec
            .args
            .iter()
            .find_map(|arg| arg.strip_prefix("--database="))
            .unwrap();
        let sql = spec.args.last().unwrap();
        self.calls.borrow_mut().push((db.into(), sql.clone()));
        let scope = if db.starts_with("source_") {
            SOURCE
        } else {
            TARGET
        };
        let stdout = if sql == evidence::verification_sql("identity").unwrap() {
            if scope == SOURCE && self.empty_source_identity.replace(false) {
                String::new()
            } else {
                format!("{UUID}\t{db}\n")
            }
        } else if sql == evidence::verification_sql("ownership").unwrap() {
            model::expected_owners(scope, db.ends_with("shared_control"))
                .into_iter()
                .map(|row| {
                    format!(
                        "{}\t{}\t{}\n",
                        row["resource_kind"].as_str().unwrap(),
                        scope,
                        row["marker"].as_str().unwrap()
                    )
                })
                .collect()
        } else if sql == "SELECT COUNT(*) FROM sys_job_schedule WHERE enabled=1 AND del_flag='0';" {
            assert_eq!(db, "target_shared_control");
            self.schedule_count
                .take()
                .expect("调度计数只能读取一次")
                .to_owned()
        } else {
            assert!(db.ends_with("shared_control"));
            if sql.starts_with("SELECT id,username,") {
                assert_eq!(
                    sql,
                    &format!(
                        "SELECT id,username,status,del_flag FROM sys_user WHERE tenant_id='system' AND id={ID};"
                    )
                );
                format!("{ID}\tfixture-private-administrator\t1\t0\n")
            } else {
                assert!(sql.contains("UTC_TIMESTAMP(6)"));
                assert!(sql.contains(&format!("WHERE tenant_id='system' AND id={ID}")));
                before().to_string()
            }
        };
        let mut bytes = stdout.into_bytes();
        if let StdoutTarget::HeldFile { file, .. } = spec.stdout {
            (&file).write_all(&bytes)?;
            bytes.clear();
        }
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt;
        Ok(CommandOutput {
            status: ExitStatus::from_raw(0),
            stdout: bytes,
            stderr: Vec::new(),
        })
    }

    fn s3(&self, _: Request) -> Result<Value> {
        panic!("调度及管理员观察不得访问对象存储")
    }
}

#[test]
fn reads_the_full_schedule_and_both_admin_rows_with_fixed_sql_and_status_only_diagnostics() {
    let fixture = Fixture::new();
    let target = fixture.storage(CloneSide::Target);
    let source = fixture.storage(CloneSide::Source);
    let fake = Fake::default();
    let initial = before();
    let action = action(&fixture.backend, &initial);
    assert_eq!(
        observe(
            &target,
            &fixture.backend,
            TARGET,
            &action,
            &fake,
            &|| Ok(())
        )
        .unwrap(),
        initial
    );
    assert_eq!(fake.calls.borrow().len(), 9);
    fake.empty_source_identity.set(true);
    verify_administrator(
        &source,
        &target,
        &json!({"subject_id":ID.to_string(),"username":"fixture-private-administrator"}),
        &fake,
        &|| Ok(()),
    )
    .unwrap();
    assert_eq!(fake.calls.borrow().len(), 16);
    assert_eq!(fake.calls.borrow()[9], fake.calls.borrow()[10]);
    verify_source_and_target(&source, &target, &fake, &|| Ok(())).unwrap();
    assert_eq!(fake.calls.borrow().len(), 32);
    super::guard::verify_target_databases(&target, &fake, &|| Ok(())).unwrap();
    assert_eq!(fake.calls.borrow().len(), 40);
    assert!(
        fake.calls.borrow()[32..]
            .iter()
            .all(|(database, _)| database.starts_with("target_"))
    );
    let mut administrator_receipts = 0;
    let mut identity_retries = 0;
    for entry in fs::read_dir(&fixture.output).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("administrator-row-") {
            administrator_receipts += 1;
            let content = fs::read_to_string(entry.path()).unwrap();
            assert!(!content.contains("fixture-private-administrator"));
            assert!(!content.contains("private-fixture-password"));
        }
        if name.starts_with("mysql-identity-retry-") {
            identity_retries += 1;
            let retry = document::read(&entry.path()).unwrap();
            let receipt = |key: &str| {
                let path = retry[key]["path"].as_str().unwrap();
                document::read(Path::new(path)).unwrap()
            };
            let empty = receipt("first");
            let verified = receipt("second");
            assert_eq!(empty["stdout_file"]["bytes"], 0);
            let path = verified["stdout_file"]["path"].as_str().unwrap();
            assert_eq!(
                fs::read_to_string(path).unwrap(),
                format!("{UUID}\tsource_shared_control\n")
            );
        }
    }
    assert_eq!(administrator_receipts, 2);
    assert_eq!(identity_retries, 1);
}

#[test]
fn seed_accepts_only_the_exact_zero_count_from_its_bound_control_database() {
    let fixture = Fixture::new();
    let target = fixture.storage(CloneSide::Target);
    for (response, allowed) in [
        ("0\r\n", true),
        ("1\n", false),
        ("00\n", false),
        ("", false),
    ] {
        let fake = Fake::default();
        fake.schedule_count.set(Some(response));
        let result = super::guard::require_disabled_schedules(&target, &fake, &|| Ok(()));
        assert_eq!(result.is_ok(), allowed);
        assert_eq!(fake.calls.borrow().len(), 3);
        assert!(
            fake.calls
                .borrow()
                .iter()
                .all(|(db, _)| db == "target_shared_control")
        );
        assert_eq!(fake.schedule_count.get(), None);
    }
    let receipts: Vec<_> = fs::read_dir(&fixture.output)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("seed-enabled-schedules-read-")
        })
        .collect();
    assert_eq!(receipts.len(), 4);
    for entry in receipts {
        assert_eq!(
            document::read(&entry.path()).unwrap(),
            json!({
                "format_version":1,"kind":"seed-enabled-schedules-read","side":"target",
                "target_key":"shared-control","returncode":0,"checkpoint_passed":true,"remote_writes":0
            })
        );
    }
}
