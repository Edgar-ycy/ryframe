//! 初始化编排的固定外部事实；复用库存替身，不访问服务或运行维护产物。
use crate::{
    Result,
    recovery::clone::native::{
        export::transport::{CommandOutput, CommandSpec, SystemTransport, Transport},
        s3::{Operation, Request},
        target_verify::{
            fresh::selection::Selected,
            prepare_observations,
            storage::{StorageInputs, StorageObserver},
        },
    },
    target_verify_fixture::{Fixture, SCOPE, UUID, database::FakeTransport, write},
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    fs,
};

use crate::target_initialize_restart_fixture as restart;

pub(super) struct Resources<'a> {
    pub(super) fixture: &'a Fixture,
    inventory: FakeTransport<'a>,
    pub(super) calls: RefCell<Vec<Vec<String>>>,
    pub(super) sql: RefCell<Vec<String>>,
    pub(super) created: RefCell<BTreeSet<String>>,
    pub(super) reset: Cell<bool>,
    pub(super) fail: Cell<Option<&'static str>>,
    redis: RefCell<BTreeMap<String, Value>>,
    plan: Value,
    reset_files: BTreeMap<String, Vec<u8>>,
    restart: RefCell<Option<restart::Restart>>,
    pub(super) receipt_failures: Cell<usize>,
    pub(super) confirmation_failure: Cell<bool>,
    pub(super) failed_publications: RefCell<Vec<std::path::PathBuf>>,
    pub(super) redis_calls: RefCell<Vec<Vec<String>>>,
    pub(super) lose_eval_reply: Cell<bool>,
}
impl<'a> Resources<'a> {
    pub(super) fn new(f: &'a Fixture) -> Self {
        let reset_files = fs::read_dir(f.target.join("reset-state"))
            .unwrap()
            .map(|v| {
                let path = v.unwrap().path();
                (
                    path.file_name().unwrap().to_str().unwrap().to_owned(),
                    fs::read(path).unwrap(),
                )
            })
            .collect();
        Self {
            fixture: f,
            inventory: FakeTransport::new(f, false, false),
            calls: RefCell::new(Vec::new()),
            sql: RefCell::new(Vec::new()),
            created: RefCell::new(BTreeSet::new()),
            reset: Cell::new(false),
            fail: Cell::new(None),
            restart: RefCell::new(None),
            receipt_failures: Cell::new(0),
            confirmation_failure: Cell::new(false),
            failed_publications: RefCell::new(Vec::new()),
            redis_calls: RefCell::new(Vec::new()),
            lose_eval_reply: Cell::new(false),
            redis: RefCell::new(BTreeMap::new()),
            reset_files,
            plan: crate::recovery::clone::native::json::read(&f.target.join("reset-plan.json"))
                .unwrap(),
        }
    }
    pub(super) fn generation(&self) -> Result<(Value, Selected, Option<Value>)> {
        self.generation_at(&self.fixture.target)
    }
    pub(super) fn generation_at(
        &self,
        directory: &std::path::Path,
    ) -> Result<(Value, Selected, Option<Value>)> {
        let restart = self.restart.borrow();
        let (value, runtime, cache) = match restart.as_ref() {
            Some(value) => (
                json!({"rustfs":value.storage["storage"],"redis":value.cache["redis"]}),
                Some(value.storage.clone()),
                Some(value.cache.clone()),
            ),
            None => (self.fixture.request["storage"].clone(), None, None),
        };
        let candidate = Selected { value, runtime };
        for slot in
            prepare_observations::slots(&self.fixture.backend, &self.fixture.request, &candidate)?
                .into_iter()
                .take(6)
        {
            write(
                &directory.join(format!(
                    "{}-{}.{}",
                    slot.name,
                    uuid::Uuid::new_v4().simple(),
                    if slot.name == "storage-layout" {
                        "json"
                    } else {
                        "command.json"
                    }
                )),
                &generation_receipt(&slot)?,
            );
        }
        use crate::recovery::clone::native::{
            bridge::scope::Scope, maintenance, target_verify::authority,
        };
        let scope = Scope::load(&self.fixture.backend, &[], &[])?;
        let mut value = authority::generation::observe(
            &self.fixture.backend,
            &self.fixture.request,
            &self.fixture.environment,
            &mut |root, path, receipt| {
                maintenance::verify_evidence(root, path, Some(receipt))?;
                scope.verify_source(
                    root,
                    path,
                    receipt,
                    receipt["source"]["worktree_fingerprint"].as_str().unwrap(),
                    None,
                )?;
                Ok(receipt.clone())
            },
            &mut |_| Ok(()),
        )?;
        value["storage"] = candidate.value.clone();
        scope.assert_unchanged()?;
        Ok((value, candidate, cache))
    }
    pub(super) fn registered_restart(&self) {
        assert!(self.reset.get());
        *self.restart.borrow_mut() = Some(restart::register(self.fixture));
        self.redis.borrow_mut().clear();
    }
    pub(super) fn eval_calls(&self) -> usize {
        self.redis_calls
            .borrow()
            .iter()
            .filter(|parts| parts.first().map(String::as_str) == Some("EVAL"))
            .count()
    }
    pub(super) fn redis_image(&self) -> Value {
        json!(*self.redis.borrow())
    }
    pub(super) fn existing_sentinel(&self) {
        let key = self.fixture.request["reset"]["sentinel_key"]
            .as_str()
            .unwrap();
        self.redis
            .borrow_mut()
            .insert(key.into(), json!("unexpected"));
    }
    pub(super) fn preflight_state(&self) {
        for db in self.fixture.request["target"]["databases"]
            .as_array()
            .unwrap()
        {
            self.created
                .borrow_mut()
                .insert(db["database"].as_str().unwrap().into());
        }
        self.redis.borrow_mut().insert(
            self.fixture.request["reset"]["sentinel_key"]
                .as_str()
                .unwrap()
                .into(),
            self.fixture.request["reset"]["sentinel_value"].clone(),
        );
    }
    pub(super) fn initialized_state(&self) {
        self.reset.set(true);
        let scope = self.fixture.request["target"]["scope_id"].as_str().unwrap();
        let mut values = self.redis.borrow_mut();
        values.insert(
            format!("ryframe:{{{scope}}}:.ryframe-owner"),
            json!(format!("ryframe-owner:v1:{scope}:redis")),
        );
        values.insert(
            self.fixture.request["reset"]["sentinel_key"]
                .as_str()
                .unwrap()
                .into(),
            self.fixture.request["reset"]["sentinel_value"].clone(),
        );
    }
    fn maintenance(&self, spec: CommandSpec) -> Result<CommandOutput> {
        assert_eq!(spec.args.first().map(String::as_str), Some("--config-dir"));
        if spec
            .args
            .get(2)
            .is_some_and(|arg| matches!(arg.as_str(), "plan" | "execute"))
        {
            let maintenance = crate::recovery::clone::native::json::read(
                &self.fixture.local.join("maintenance/build.json"),
            )?;
            let snapshot = crate::recovery::clone::native::target_verify::fresh::initialize::reset_configuration::Snapshot::open(
                &self.fixture.backend, &self.fixture.target, &self.fixture.request, &maintenance,
            )?;
            assert_eq!(
                spec.args.get(1).map(String::as_str),
                snapshot.directory.to_str()
            );
        } else {
            assert_eq!(
                spec.args.get(1).map(String::as_str),
                self.fixture.request["config_dir"].as_str()
            );
        }
        let mut spec = spec;
        let arguments = spec.args.split_off(2);
        spec.args = arguments;
        if self.fail.get() == Some(spec.label.as_str()) {
            self.fail.set(None);
            return Err(std::io::Error::from(std::io::ErrorKind::NotFound).into());
        }
        if spec.args == ["plan"] {
            return Ok(output(format!(
                "{}\nplan_hash={}\n",
                self.plan["manifest"],
                self.plan["plan_hash"].as_str().unwrap()
            )));
        }
        if spec.args.first().is_some_and(|v| v == "execute") {
            assert!(self.fixture.target.join("reset.intent.json").is_file());
            let now = chrono::Utc::now().to_rfc3339();
            for (name, bytes) in &self.reset_files {
                let mut value: Value = serde_json::from_slice(bytes)?;
                reset_times(&mut value, &now);
                fs::write(
                    self.fixture.target.join("reset-state").join(name),
                    serde_json::to_vec_pretty(&value)?,
                )?;
            }
            self.reset.set(true);
            let scope = self.fixture.request["target"]["scope_id"].as_str().unwrap();
            self.redis.borrow_mut().insert(
                format!("ryframe:{{{scope}}}:.ryframe-owner"),
                json!(format!("ryframe-owner:v1:{scope}:redis")),
            );
            let done = crate::recovery::clone::native::target_verify::reset::completed(
                &self.fixture.backend,
                &self.fixture.target,
                &self.plan["manifest"],
                self.plan["plan_hash"].as_str().unwrap(),
            )?;
            return Ok(output(format!(
                "reset_status=completed\nreset_completed_at={}\nreset_report={}\n",
                done["completed_at"].as_str().unwrap(),
                self.fixture
                    .target
                    .join(done["report"]["file"].as_str().unwrap())
                    .display()
            )));
        }
        let sequence = crate::recovery::clone::native::target_verify::resume_prefix::operations(
            spec.program.to_str().unwrap(),
            &self.fixture.request,
        )?;
        let operation = sequence["operations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| {
                v["command"].as_array().unwrap().iter().skip(3).eq(spec
                    .args
                    .iter()
                    .map(|v| json!(v))
                    .collect::<Vec<_>>()
                    .iter())
            })
            .ok_or("替身拒绝未知维护argv")?;
        Ok(output(operation["stdout"].as_str().unwrap().to_owned()))
    }
    fn mysql(&self, spec: CommandSpec) -> Result<CommandOutput> {
        let sql = String::from_utf8(spec.stdin.unwrap())?;
        self.sql.borrow_mut().push(sql.clone());
        let db = self.fixture.request["target"]["databases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|db| sql.contains(db["database"].as_str().unwrap()))
            .ok_or("替身拒绝未知库")?;
        let name = db["database"].as_str().unwrap();
        if sql.starts_with("DROP DATABASE") {
            assert!(
                self.fixture
                    .target
                    .join("reconciliation-plan.json")
                    .is_file()
            );
            assert!(self.created.borrow_mut().remove(name));
            if self.fail.get() == Some("drop") {
                return Err("模拟DROP已提交但回复丢失；私有凭据不得进入公开收据".into());
            }
            return Ok(output(String::new()));
        }
        if sql.starts_with("CREATE DATABASE") {
            assert!(
                self.fixture
                    .target
                    .join(format!(
                        "create-{}.intent.json",
                        db["key"].as_str().unwrap()
                    ))
                    .is_file()
            );
            assert!(self.created.borrow_mut().insert(name.into()));
            if self.fail.get() == Some("create") {
                self.fail.set(None);
                return Err("模拟CREATE已提交但结果未知".into());
            }
            return Ok(output(String::new()));
        }
        if sql.starts_with("SELECT TABLE_NAME") {
            return Ok(output(if self.fail.get() == Some("nonempty") {
                "unknown_table".into()
            } else {
                String::new()
            }));
        }
        assert!(sql.starts_with("SELECT @@server_uuid; SELECT SCHEMA_NAME"));
        Ok(output(if self.created.borrow().contains(name) {
            format!("{UUID}\n{name}\n")
        } else {
            format!("{UUID}\n")
        }))
    }
}
impl Transport for Resources<'_> {
    fn publish_evidence(&self, path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
        let name = path.file_name().and_then(|v| v.to_str()).unwrap();
        if name.starts_with("migrate-tenant-data---target-dedicated-a-verify-")
            && self.receipt_failures.get() > 0
        {
            self.receipt_failures.set(self.receipt_failures.get() - 1);
            self.failed_publications.borrow_mut().push(path.to_owned());
            return Err(std::io::ErrorKind::NotFound.into());
        }
        if name.starts_with("resume-migrate-")
            && name.ends_with("-04.confirmed.json")
            && self.confirmation_failure.replace(false)
        {
            self.failed_publications.borrow_mut().push(path.to_owned());
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        SystemTransport.publish_evidence(path, bytes)
    }

    fn run(&self, spec: CommandSpec) -> Result<CommandOutput> {
        let mut call = vec![spec.program.to_string_lossy().into_owned()];
        call.extend(spec.args.clone());
        self.calls.borrow_mut().push(call);
        if spec.stdin.is_some() {
            return self.mysql(spec);
        }
        if spec.args.get(2).is_some_and(|v| v == "target-inventory")
            || spec.args.iter().any(|v| v == "--execute")
        {
            if self.fail.get() == Some("inventory-model")
                && spec
                    .args
                    .get(2)
                    .is_some_and(|arg| arg == "target-inventory")
            {
                self.fail.set(None);
                let path = std::path::PathBuf::from(&spec.args[6]);
                let output = self.inventory.run(spec)?;
                let mut raw = crate::recovery::clone::native::json::read(&path)?;
                raw["target"]["preserved_tables"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|entry| entry["table"] != "biz_data_scope_projection");
                fs::write(path, serde_json::to_vec(&raw)?)?;
                return Ok(output);
            }
            if self.fail.get() == Some("inventory") {
                self.fail.set(None);
                return Err(
                    "模拟只读库存暂不可用 mysql://user:inventory-private-secret@host/database"
                        .into(),
                );
            }
            return self.inventory.run(spec);
        }
        self.maintenance(spec)
    }
    fn s3(&self, request: Request) -> Result<Value> {
        if self.fail.get() == Some("objects") {
            return Err("模拟未知对象前像".into());
        }
        if !self.reset.get() {
            return match request.operation {
                Operation::HeadBucket | Operation::Versioning => Ok(json!({})),
                Operation::List { .. } => Ok(json!({"Contents":[],"IsTruncated":false})),
                _ => Err("reset前不得读取owner或写对象".into()),
            };
        }
        let destination = match &request.operation {
            Operation::Get { destination, .. } => Some(destination.clone()),
            _ => None,
        };
        let mut value = self.inventory.s3(request)?;
        if let Some(path) = destination {
            value = json!({"ContentLength":fs::metadata(path)?.len(),"ETag":"\"owner\"","ContentType":"text/plain","LastModified":"2026-10-26T00:00:00Z","Metadata":{}});
        }
        Ok(value)
    }
}
impl StorageObserver for Resources<'_> {
    fn process_identity(&self, _: u32) -> Result<Option<Value>> {
        panic!("编排替身不伪造实际进程观察")
    }
    fn arguments(&self, _: &StorageInputs<'_>, _: &Value) -> Result<Vec<String>> {
        panic!("编排替身不伪造argv")
    }
    fn listener(&self, _: &StorageInputs<'_>, _: &Value, _: &str) -> Result<()> {
        panic!("编排替身不伪造监听事实")
    }
    fn wsl(&self, _: &StorageInputs<'_>, _: &Value, _: &[String]) -> Result<String> {
        panic!("编排替身不探测WSL")
    }
    fn redis(&self, inputs: &StorageInputs<'_>, _: &Value, parts: &[&str]) -> Result<Value> {
        self.redis_calls
            .borrow_mut()
            .push(parts.iter().map(|value| (*value).to_owned()).collect());
        let mut values = self.redis.borrow_mut();
        match parts[0] {
            "SCAN" => Ok(json!([
                "0",
                values
                    .keys()
                    .filter(|v| v.starts_with(&format!("ryframe:{{{SCOPE}}}:")))
                    .collect::<Vec<_>>()
            ])),
            "GET" => Ok(values.get(parts[1]).cloned().unwrap_or(Value::Null)),
            "DEL" => {
                assert!(
                    self.fixture
                        .target
                        .join("reconciliation-plan.json")
                        .is_file()
                );
                assert_eq!(
                    parts[1],
                    inputs.request["reset"]["sentinel_key"].as_str().unwrap()
                );
                let removed = u8::from(values.remove(parts[1]).is_some());
                Ok(match self.fail.get() {
                    Some("del-numeric") => json!(removed),
                    Some("del-unconfirmed") => json!("0"),
                    _ => json!(removed.to_string()),
                })
            }
            "EVAL" => {
                assert_eq!(parts[2], "2");
                assert_eq!(parts[3], format!("ryframe:{{{SCOPE}}}:.ryframe-owner"));
                assert_eq!(
                    parts[4],
                    inputs.request["reset"]["sentinel_key"].as_str().unwrap()
                );
                assert!(values.is_empty(), "原子marker输入必须是完整空前像");
                values.insert(parts[3].into(), json!(parts[6]));
                values.insert(parts[4].into(), json!(parts[7]));
                if self.lose_eval_reply.replace(false) {
                    return Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into());
                }
                Ok(json!("2"))
            }
            "SET" => {
                assert_eq!(parts[3], "NX");
                assert!(self.fixture.target.join("sentinel.intent.json").is_file());
                if values.contains_key(parts[1]) {
                    return Ok(Value::Null);
                }
                values.insert(parts[1].into(), json!(parts[2]));
                Ok(json!("OK"))
            }
            _ => Err(format!(
                "编排替身拒绝未声明Redis操作；scope={}",
                inputs.request["target"]["scope_id"]
            )
            .into()),
        }
    }
}
fn output(stdout: String) -> CommandOutput {
    #[cfg(windows)]
    let status = {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(0)
    };
    #[cfg(unix)]
    let status = {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(0)
    };
    CommandOutput {
        status,
        stdout: stdout.into_bytes(),
        stderr: Vec::new(),
    }
}
pub(super) fn retain_prepared(f: &Fixture) {
    let generation = crate::recovery::clone::native::json::read(&f.target.join("prepare.json"))
        .unwrap()["generation"]
        .clone();
    let snapshot = crate::recovery::clone::native::target_verify::fresh::initialize::reset_configuration::Snapshot::open(
        &f.backend, &f.target, &f.request, &generation["maintenance"],
    ).unwrap();
    let directory = snapshot.directory.clone();
    drop(snapshot);
    assert!(directory.starts_with(f.backend.join(".local-tests")));
    fs::remove_dir_all(directory).unwrap();
    let baseline =
        crate::recovery::clone::native::json::read(&f.local.join("prepared-files.json")).unwrap();
    let tree =
        crate::recovery::clone::native::target_verify::tree::snapshot(&f.backend, &f.target, false)
            .unwrap();
    for item in tree.as_array().unwrap().iter().rev() {
        if baseline["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["path"] == item["path"])
        {
            continue;
        }
        let path = f.target.join(item["path"].as_str().unwrap());
        if item["type"] == "directory" {
            fs::remove_dir(path).unwrap();
        } else {
            fs::remove_file(path).unwrap();
        }
    }
    fs::remove_file(f.local.join("initialized-files.json")).unwrap();
}

fn reset_times(value: &mut Value, now: &str) {
    if let Some(fields) = value.as_object_mut() {
        for (key, value) in fields {
            if matches!(key.as_str(), "started_at" | "completed_at" | "updated_at")
                && !value.is_null()
            {
                *value = json!(now);
            } else {
                reset_times(value, now);
            }
        }
    }
}

// slot是匹配规则，只有完整命令输出才是可供历史校验消费的收据。
fn generation_receipt(slot: &prepare_observations::Slot) -> Result<Value> {
    if slot.name == "storage-layout" {
        return Ok(slot.expected.clone());
    }
    let kernel = &slot.expected["kernel"];
    let stdout = if kernel.get("pid").is_some() {
        let mut fields = vec!["1".to_owned(); 20];
        fields[19] = kernel["started"]
            .as_str()
            .ok_or("夹具缺少Redis创建代次")?
            .to_owned();
        format!("{} (redis) {}\n", kernel["pid"], fields.join(" "))
    } else if let Some(text) = kernel.get("text").and_then(Value::as_str) {
        format!("{text}\n")
    } else {
        format!(
            "{}  {}\n",
            kernel["sha"].as_str().ok_or("夹具缺少固定SHA")?,
            kernel["file"].as_str().ok_or("夹具缺少固定文件")?
        )
    };
    let value = json!({"command":slot.expected["command"],"returncode":0,"error_type":null,"stdout":stdout,"stderr":""});
    prepare_observations::matches_slot(&value, &slot.expected, true)?;
    Ok(value)
}
