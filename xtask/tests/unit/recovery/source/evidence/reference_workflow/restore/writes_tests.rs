use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    fs,
    path::PathBuf,
    process::ExitStatus,
};

use serde_json::json;

use super::super::super::tests::{formalize, temporary};
use super::*;
use crate::recovery::{
    clone::native::{
        export::transport::{CommandOutput, CommandSpec},
        json as document,
        resources::common,
        s3,
    },
    fixture_control::portable_path,
    inputs::domain::BUCKETS,
    runtime::restore_guard::with_fixture_owner,
};

const DATABASES: [(&str, &str, &str); 4] = [
    ("shared-control", "combined", "shared"),
    ("shared", "tenant", "shared"),
    ("dedicated-b", "tenant", "dedicated"),
    ("dedicated-a", "tenant", "dedicated"),
];

const OBJECT_KEYS: [&str; 2] = ["target/tenant/item-1", "target/tenant/item-2"];
const OWNER: &str = "restore-runtime-registration.lock/owner.json";

struct Fixture {
    _directory: tempfile::TempDir,
    backend: PathBuf,
    control: PathBuf,
    work: PathBuf,
    backup: PathBuf,
    plan: Value,
    manifest: Value,
    request: Value,
    environment: BTreeMap<String, String>,
    binding: Value,
    tasks: Value,
}

impl Fixture {
    fn new() -> Self {
        let directory = temporary();
        let backend = portable_path(&directory.path().canonicalize().unwrap());
        let control = backend.join(".local-tests");
        let work = control.join("write-evidence");
        let backup = control.join("backup");
        let private = control.join("mysql-temp");
        for dir in [&work, &backup.join("databases"), &private] {
            fs::create_dir_all(dir).unwrap();
        }
        let plan = plan(&control);
        let manifest = manifest(&backup, &plan);
        let tasks =
            model::resources(&json!({"plan":plan,"root":backup,"manifest":manifest})).unwrap();
        let target = control.join("target-plan.json");
        document::write_new(&target, &json!({"fixture":"target-plan"})).unwrap();
        let registration = control.join("registration.json");
        let target = common::binding(&target).unwrap();
        document::write_new(&registration, &json!({"target_plan":target})).unwrap();
        let binding =
            json!({"registration":common::binding(&registration).unwrap(),"target_plan":target});
        let request = json!({"target":plan["target"]});
        let environment = BTreeMap::from([("TEMP".into(), private.to_str().unwrap().to_owned())]);
        Self {
            _directory: directory,
            backend,
            control,
            work,
            backup,
            plan,
            manifest,
            request,
            environment,
            binding,
            tasks,
        }
    }

    fn run(&self, mock: &Mock<'_>) -> Result<()> {
        let writes = Writes {
            storage: StorageInputs {
                backend: &self.backend,
                request: &self.request,
                environment: &self.environment,
                observation_dir: &self.work,
            },
            work: &self.work,
            backup_root: &self.backup,
            plan: &self.plan,
            manifest: &self.manifest,
        };
        let result = with_fixture_owner(
            &self.control,
            &self.binding,
            || mock.verify(),
            |checkpoint| {
                writes.run(&self.tasks, mock, |request| mock.put(request), &|| {
                    assert_eq!(checkpoint()?, self.binding);
                    Ok(())
                })
            },
        );
        assert!(!self.control.join(OWNER).exists());
        result
    }

    fn diagnostics(&self, prefix: &str) -> Result<Vec<Value>> {
        let mut diagnostics = Vec::new();
        for entry in fs::read_dir(&self.work)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_str().unwrap();
            if name.starts_with(prefix) && name.ends_with(".diagnostic.json") {
                let value = document::read(&entry.path())?;
                assert_eq!(value["automatic_retry_permitted"], false);
                let raw = value.to_string();
                assert!(!raw.contains("夹具私有命令输出"));
                assert!(!raw.contains("fixture-secret"));
                diagnostics.push(value);
            }
        }
        Ok(diagnostics)
    }

    fn sql_intent(&self, diagnostic: &Value) -> Result<Value> {
        let path = common::bound_file(&self.backend, &diagnostic["intent"], "夹具写意图")?;
        let value = document::read(&path)?;
        assert_eq!(value["kind"], "restore-reference-mysql-intent");
        assert_eq!(value["automatic_retry_permitted"], false);
        assert!(!value.to_string().contains("INSERT INTO"));
        Ok(value)
    }
}

fn plan(control: &Path) -> Value {
    let databases = |side: &str| {
        DATABASES
            .iter()
            .rev()
            .map(|(key, kind, mode)| {
                json!({"key":key,"kind":kind,"mode":mode,"database":database_name(side, key),
                    "server_uuid":format!("{side}-server")})
            })
            .collect::<Vec<_>>()
    };
    let mut plan = json!({"id":"write-chain","source":{"scope_id":"source","databases":databases("source")},
        "target":{"scope_id":"target","databases":databases("target")}});
    formalize(&mut plan, control);
    let defaults = control.join("client.cnf");
    fs::write(
        &defaults,
        "[client]\nhost=127.0.0.1\nport=3307\nuser=fixture\npassword=fixture-private\n",
    )
    .unwrap();
    let defaults_digest = common::binding(&defaults).unwrap();
    for side in ["source", "target"] {
        let databases = plan[side]["databases"].as_array_mut().unwrap();
        for database in databases {
            database["defaults_file"] = json!(defaults);
            database["defaults_sha256"] = defaults_digest["sha256"].clone();
        }
    }
    plan
}

fn database_name(side: &str, key: &str) -> String {
    format!("{side}_{}", key.replace('-', "_"))
}

fn sql_dump(id: usize) -> String {
    format!("-- ryframe-reference-data-v1\nINSERT INTO `sys_post` (`id`) VALUES ({id});\n")
}

fn artifact(root: &Path, relative: &str, resource: &str) -> Value {
    let binding = common::binding(&root.join(relative)).unwrap();
    json!({"relative_path":relative,"resource":resource,
        "bytes":binding["bytes"],"sha256":binding["sha256"]})
}

fn manifest(root: &Path, plan: &Value) -> Value {
    let mut artifacts = Vec::new();
    let mut databases = Vec::new();
    for (index, (key, kind, mode)) in DATABASES.into_iter().enumerate() {
        let relative = format!("databases/{key}.sql");
        fs::write(root.join(&relative), sql_dump(index + 1)).unwrap();
        artifacts.push(artifact(root, &relative, &format!("db:{key}")));
        databases.push(
            json!({"key":key,"database":database_name("source",key),"kind":kind,
            "server_uuid":"source-server","shared":mode=="shared","placements":[],
            "tables":[{"table":"sys_post","rows":1,"sha256":"a".repeat(64)}]}),
        );
    }
    let objects = object_buckets(root, &mut artifacts);
    json!({"id":plan["id"],"scope_id":"source","source_sha":"a".repeat(40),
        "quiesced_at":"2026-11-13T00:00:00Z","captured_at":"2026-11-13T00:00:01Z",
        "control_schema_fingerprint":"b".repeat(16),"tenant_schema_fingerprint":"c".repeat(64),
        "databases":databases,"objects":objects,"artifacts":artifacts})
}

fn object_buckets(root: &Path, artifacts: &mut Vec<Value>) -> Vec<Value> {
    let mut objects = Vec::new();
    for bucket in BUCKETS {
        fs::create_dir_all(root.join("objects").join(bucket)).unwrap();
        let mut entries = Vec::new();
        let mut index = Vec::new();
        let resource = format!("objects:{bucket}");
        if bucket == "uploads" {
            for id in 1..=OBJECT_KEYS.len() {
                let relative = format!("objects/{bucket}/item-{id}.bin");
                fs::write(root.join(&relative), format!("对象夹具 {id}")).unwrap();
                let artifact = artifact(root, &relative, &resource);
                let entry = json!({"key":format!("source/tenant/item-{id}"),
                    "bytes":artifact["bytes"],"sha256":artifact["sha256"]});
                entries.push(entry.clone());
                let mut indexed = entry;
                indexed["file"] = json!(relative);
                indexed["content_type"] = json!("application/octet-stream");
                index.push(indexed);
                artifacts.push(artifact);
            }
        }
        let relative = format!("objects/{bucket}/index.json");
        document::write_new(&root.join(&relative), &json!({"entries":index})).unwrap();
        artifacts.push(artifact(root, &relative, &resource));
        objects.push(json!({"bucket":bucket,"prefix":"source/","entries":entries}));
    }
    objects
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    Complete,
    UnknownSql,
    UnknownPut,
    ArtifactDrift,
    DeniedCheckpoint,
}

struct Mock<'a> {
    fixture: &'a Fixture,
    scenario: Scenario,
    sent: RefCell<Vec<String>>,
    checked_pending: RefCell<Option<PathBuf>>,
    denied: Cell<bool>,
}

impl<'a> Mock<'a> {
    fn new(fixture: &'a Fixture, scenario: Scenario) -> Self {
        Self {
            fixture,
            scenario,
            sent: RefCell::new(Vec::new()),
            checked_pending: RefCell::new(None),
            denied: Cell::new(false),
        }
    }

    fn check_owner(&self) -> Result<()> {
        let owner = document::read(&self.fixture.control.join(OWNER))?;
        assert_eq!(
            owner["operation"],
            format!(
                "restore:{}:{}",
                self.fixture.binding["registration"]["sha256"]
                    .as_str()
                    .unwrap(),
                self.fixture.binding["target_plan"]["sha256"]
                    .as_str()
                    .unwrap(),
            )
        );
        Ok(())
    }

    fn verify(&self) -> Result<()> {
        self.check_owner()?;
        let sql = self.fixture.diagnostics("restore-mysql-")?;
        let put = self.fixture.diagnostics("object-create-")?;
        let sql = sql
            .iter()
            .filter(|row| row["status"] == "pending")
            .map(|row| row["intent"]["path"].as_str().unwrap());
        let put = put
            .iter()
            .filter(|row| row["error_type"] == "WriteResultNotRecorded")
            .map(|row| row["request_file"].as_str().unwrap());
        let mut pending = sql.chain(put);
        let checked = pending.next().map(PathBuf::from);
        assert!(pending.next().is_none());
        if self.scenario == Scenario::DeniedCheckpoint
            && checked.is_some()
            && !self.denied.replace(true)
        {
            return Err("模拟 SQL 发送前原代次漂移".into());
        }
        *self.checked_pending.borrow_mut() = checked;
        Ok(())
    }

    fn note_send(&self, intent: &Path, label: String) -> Result<()> {
        self.check_owner()?;
        assert_eq!(
            self.checked_pending.borrow_mut().take().as_deref(),
            Some(intent)
        );
        self.sent.borrow_mut().push(label);
        Ok(())
    }

    fn put(&self, request: PutRequest) -> Result<Value> {
        let pending = self.fixture.diagnostics("object-create-")?;
        let pending = pending
            .iter()
            .find(|row| row["error_type"] == "WriteResultNotRecorded")
            .unwrap();
        let intent_path = Path::new(pending["request_file"].as_str().unwrap());
        let intent = document::read(intent_path)?;
        assert_eq!(intent["scope_id"], "target");
        assert_eq!(intent["bucket"], "uploads");
        assert_eq!(intent["key"], request.key);
        assert_eq!(intent["expected"]["bytes"], request.expected_bytes);
        assert_eq!(intent["expected"]["sha256"], request.expected_sha256);
        let number = self.sent.borrow().len() - DATABASES.len();
        assert_eq!(request.bucket, "uploads");
        assert_eq!(request.key, OBJECT_KEYS[number]);
        assert_eq!(request.metadata["ContentType"], "application/octet-stream");
        assert_eq!(
            fs::read(&request.source)?,
            format!("对象夹具 {}", number + 1).as_bytes(),
        );
        self.note_send(intent_path, request.key)?;
        if self.scenario == Scenario::UnknownPut {
            return Err("模拟条件创建响应中断".into());
        }
        Ok(
            json!({"status":"created_unverified","http_status":200,"remote_write_may_have_occurred":true,
            "automatic_retry_permitted":false,"response":{"ETag":"object"}}),
        )
    }
}

impl Transport for Mock<'_> {
    fn run(&self, _: CommandSpec) -> Result<CommandOutput> {
        panic!("参考恢复不得使用普通 SQL 传输");
    }

    fn run_private(&self, command: CommandSpec) -> Result<CommandOutput> {
        let pending = self.fixture.diagnostics("restore-mysql-")?;
        let pending = pending
            .iter()
            .find(|row| row["status"] == "pending")
            .unwrap();
        let intent = self.fixture.sql_intent(pending)?;
        let index = self.sent.borrow().len();
        let key = DATABASES[index].0;
        let database = database_name("target", key);
        assert_eq!(intent["target_key"], key);
        assert_eq!(intent["target_database"], database);
        assert_eq!(intent["relative"], format!("databases/{key}.sql"));
        assert_eq!(command.args[1], format!("--database={database}"));
        let sql = String::from_utf8(command.stdin.unwrap()).unwrap();
        assert!(sql.contains(&sql_dump(index + 1)));
        assert!(sql.ends_with("\nCOMMIT;\n"));
        self.note_send(
            Path::new(pending["intent"]["path"].as_str().unwrap()),
            database,
        )?;
        if self.scenario == Scenario::UnknownSql {
            return Err("夹具私有命令输出".into());
        }
        if self.scenario == Scenario::ArtifactDrift && index == 0 {
            let next = self.fixture.backup.join("databases/shared.sql");
            fs::write(next, sql_dump(9))?;
        }
        Ok(CommandOutput {
            status: success_status(),
            stdout: "夹具私有命令输出".as_bytes().to_vec(),
            stderr: Vec::new(),
        })
    }

    fn s3(&self, _: s3::Request) -> Result<Value> {
        panic!("参考恢复测试只接受条件创建端口");
    }
}

fn success_status() -> ExitStatus {
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(0)
}

#[test]
fn writes_four_databases_in_inventory_order_then_two_objects_under_registration_owner() {
    let fixture = Fixture::new();
    let mock = Mock::new(&fixture, Scenario::Complete);
    fixture.run(&mock).unwrap();
    let expected = DATABASES
        .into_iter()
        .map(|(key, _, _)| database_name("target", key))
        .chain(OBJECT_KEYS.map(str::to_owned))
        .collect::<Vec<_>>();
    assert_eq!(*mock.sent.borrow(), expected);
    let sql = fixture.diagnostics("restore-mysql-").unwrap();
    let objects = fixture.diagnostics("object-create-").unwrap();
    assert_eq!(sql.len(), 4);
    assert!(sql.iter().all(|row| row["status"] == "exited_zero"));
    assert_eq!(objects.len(), 2);
    assert!(
        objects
            .iter()
            .all(|row| row["outcome"] == "created_unverified")
    );
}

#[test]
fn unknown_first_sql_or_put_stops_later_writes_and_preserves_diagnostics() {
    for (scenario, sql_count, put_count, prefix, field, status) in [
        (
            Scenario::UnknownSql,
            1,
            0,
            "restore-mysql-",
            "status",
            "unknown",
        ),
        (
            Scenario::UnknownPut,
            4,
            1,
            "object-create-",
            "outcome",
            "needs_reconciliation",
        ),
    ] {
        let fixture = Fixture::new();
        let mock = Mock::new(&fixture, scenario);
        let error = fixture.run(&mock).unwrap_err().to_string();
        assert!(error.contains("禁止") && !error.contains("夹具私有命令输出"));
        let sql = fixture.diagnostics("restore-mysql-").unwrap();
        let puts = fixture.diagnostics("object-create-").unwrap();
        assert_eq!((sql.len(), puts.len()), (sql_count, put_count));
        assert_eq!(mock.sent.borrow().len(), sql_count + put_count);
        let diagnostic = fixture.diagnostics(prefix).unwrap();
        assert_eq!(diagnostic.len(), 1);
        assert_eq!(diagnostic[0][field], status);
        assert_eq!(diagnostic[0]["remote_write_may_have_occurred"], true);
    }
}

#[test]
fn changed_second_sql_artifact_stops_after_first_database() {
    let fixture = Fixture::new();
    let mock = Mock::new(&fixture, Scenario::ArtifactDrift);
    let error = fixture.run(&mock).unwrap_err().to_string();
    assert!(error.contains("当前产物字节与备份清单不同"), "{error}");
    assert_eq!(*mock.sent.borrow(), ["target_shared_control"]);
    let sql = fixture.diagnostics("restore-mysql-").unwrap();
    assert_eq!(sql.len(), 1);
    assert_eq!(sql[0]["status"], "exited_zero");
    assert_eq!(
        fs::read_to_string(fixture.backup.join("databases/shared.sql")).unwrap(),
        sql_dump(9)
    );
    assert!(fixture.diagnostics("object-create-").unwrap().is_empty());
}

#[test]
fn final_sql_checkpoint_denial_after_intent_records_not_started_without_writes() {
    let fixture = Fixture::new();
    let mock = Mock::new(&fixture, Scenario::DeniedCheckpoint);
    let error = fixture.run(&mock).unwrap_err().to_string();
    assert!(error.contains("已保存未启动诊断"), "{error}");
    assert!(mock.denied.get());
    assert!(mock.sent.borrow().is_empty());
    let sql = fixture.diagnostics("restore-mysql-").unwrap();
    assert_eq!(sql.len(), 1);
    assert_eq!(sql[0]["status"], "not_started");
    assert_eq!(sql[0]["remote_write_may_have_occurred"], false);
    assert_eq!(
        fixture.sql_intent(&sql[0]).unwrap()["target_key"],
        DATABASES[0].0
    );
    assert!(fixture.diagnostics("object-create-").unwrap().is_empty());
}
