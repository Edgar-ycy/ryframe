use super::super::super::{
    json as doc,
    lifecycle::{lock::RunLock, state},
    resources::common,
};
use super::super::{
    markers,
    session::Runtime,
    stage::{self},
};
use crate::Result;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(super) struct Fixture {
    pub root: tempfile::TempDir,
    pub directory: PathBuf,
    pub request: Value,
    pub request_file: PathBuf,
    pub ports: Ports,
    pub backend: PathBuf,
}
pub(super) struct Ports {
    pub store: BTreeMap<String, Value>,
    pub commands: Vec<Value>,
    pub alive: bool,
    pub counter: u64,
    pub failure: Option<&'static str>,
    pub guard_failure: bool,
    pub validate_failure: bool,
    pub stop_calls: u64,
    pub partial: bool,
    pub process: Value,
    pub source: Value,
    pub observe_calls: u64,
    pub selected: Value,
    pub boundaries: Value,
    pub private: Value,
}
impl Fixture {
    pub fn new() -> Self {
        let backend = crate::workspace::root_dir();
        fs::create_dir_all(backend.join(".local-tests/tmp")).unwrap();
        let root = tempfile::Builder::new()
            .prefix("cache-stage-")
            .tempdir_in(backend.join(".local-tests/tmp"))
            .unwrap();
        let directory = root.path().join("run");
        fs::create_dir(&directory).unwrap();
        let marker = json!({"namespace":"ryframe:{scope-seed}:","ownership_key":"ryframe:{scope-seed}:.ryframe-owner","ownership_value":"owned","sentinel_key":"ryframe:devex-fresh:scope-seed:sentinel","sentinel_value":"devex-fresh:fixture"});
        let previous = json!({"port":16390,"wsl":{"path":"wsl.exe","sha256":"a".repeat(64)},"configuration":{"sha256":"b".repeat(64)},"pid":610000,"started":"100","run_id":"a".repeat(40)});
        let manifest = json!({"initialized":{"path":"not-observed-during-cleanup"},"target_environment":{"path":"not-observed-during-cleanup"}});
        doc::write_new(&directory.join("manifest.json"), &manifest).unwrap();
        state::initialize(&directory).unwrap();
        let process = json!({"password":"fixture-secret","wsl":previous["wsl"],"python":{"path":"/usr/bin/python3","sha256":"c".repeat(64)}});
        let request = json!({"format_version":1,"kind":"devex-clone-cache-restart","side":"target","manifest":common::binding(&directory.join("manifest.json")).unwrap(),"previous":previous,"process":process,"markers":marker});
        let request_file = root.path().join("request.json");
        doc::write_new(&request_file, &request).unwrap();
        Self {
            root,
            directory,
            request,
            request_file,
            backend,
            ports: Ports {
                store: BTreeMap::new(),
                commands: vec![],
                alive: false,
                counter: 0,
                failure: None,
                guard_failure: false,
                validate_failure: false,
                stop_calls: 0,
                partial: false,
                process: Value::Null,
                source: Value::Null,
                observe_calls: 0,
                selected: Value::Null,
                boundaries: json!({"later":[],"segment":null}),
                private: json!({"APP_ENV":"test"}),
            },
        }
    }
    pub fn call(&mut self, action: &str, parameters: Value) -> Result<Value> {
        stage::dispatch(
            &self.backend,
            &json!({"action":action,"directory":self.directory,"caller":std::process::id(),"parameters":parameters}),
            &mut self.ports,
        )
    }
    pub fn execute(&mut self, mode: &str, outer_failure: bool) -> Result<Value> {
        let lock = RunLock::claim(&self.directory)?;
        let (num, attempt) = state::begin(&self.directory, "cache-target", mode, &json!({}))?;
        state::bind_controller(&self.directory, num, &lock.owner, &attempt)?;
        let result=self.call("execute",json!({"value":doc::read(&self.directory.join("manifest.json"))?,"mode":mode,"number":num,"request_file":if mode=="restart"{json!(self.request_file)}else{Value::Null}}));
        match &result {
            Ok(value) => state::finish(
                &self.directory,
                num,
                Some(value),
                if outer_failure {
                    Some("ValueError")
                } else {
                    None
                },
            )?,
            Err(_) => state::finish(&self.directory, num, None, Some("ValueError"))?,
        }
        lock.release()?;
        result
    }
    pub fn binding(&mut self) -> Result<Value> {
        self.call(
            "binding",
            json!({"owned_lock_identity":null,"before":null,"frozen":null}),
        )
    }
    pub fn status(&mut self) -> Result<Value> {
        self.call("status", json!({"allow_close_attempt":false}))
    }
    pub fn output(&self, num: u64) -> PathBuf {
        self.directory
            .join("cache-target")
            .join(format!("a{num:04}"))
    }
    pub fn writes(&self) -> usize {
        self.ports
            .commands
            .iter()
            .filter(|r| r[0] == "MSETNX")
            .count()
    }
    pub fn overwrite(&self, path: &Path, value: &Value) {
        doc::write_replace(path, value).unwrap();
    }
    pub fn seed(&mut self) -> Value {
        self.execute("restart", false).unwrap();
        self.ports.alive = false;
        let lock = RunLock::claim(&self.directory).unwrap();
        let (n, _) = state::begin(
            &self.directory,
            "seed-runtime",
            "source-register",
            &json!({}),
        )
        .unwrap();
        state::finish(
            &self.directory,
            n,
            Some(&json!({"status":"seed_source_registered"})),
            None,
        )
        .unwrap();
        lock.release().unwrap();
        self.ports.source = json!({"review_successor":{"source_result":common::binding(&state::result_path(&self.directory,n)).unwrap()}});
        self.ports.process = self.request["process"].clone();
        self.ports.process["python"]["sha256"] = json!("d".repeat(64));
        let file = self.root.path().join("successor.json");
        doc::write_new(&file, &json!({"kind":"devex-clone-seed-review-successor"})).unwrap();
        self.request_file = file.clone();
        common::binding(&file).unwrap()
    }
}
impl Ports {
    fn observed(&self, request: &Value, base: &Value) -> Result<Value> {
        if !self.alive {
            return Err("fixture stopped".into());
        }
        let generation = base["generation"]
            .as_u64()
            .ok_or("fixture generation missing")?;
        let mut runtime = base.clone();
        runtime["redis"] = self.selected.clone();
        runtime["redis"]["wsl"] = request["wsl"].clone();
        runtime["redis"]["pid"] = json!(620000 + generation);
        runtime["redis"]["started"] = json!((200 + generation).to_string());
        runtime["redis"]["run_id"] = json!(generation.to_string().repeat(40));
        runtime["facts"] = json!({});
        Ok(runtime)
    }
}
impl Runtime for Ports {
    fn call(&mut self, operation: &str, a: Value) -> Result<Value> {
        match operation {
            "validate" => {
                if self.validate_failure {
                    return Err("current product validation forbidden".into());
                }
                self.selected = a["request"]["previous"].clone();
                Ok(self.private.clone())
            }
            "successor" => Ok(json!([self.process, self.private, self.source])),
            "producers-stopped" => {
                if self.guard_failure {
                    Err("producer alive".into())
                } else {
                    Ok(Value::Null)
                }
            }
            "quiet" | "restart-authorization" => Ok(Value::Null),
            "seed-history" => Ok(self.boundaries["later"].clone()),
            "seed-segment" => Ok(self.boundaries["segment"].clone()),
            "inspect" => {
                let path = PathBuf::from(a["output"].as_str().unwrap()).join("base.json");
                if path.exists() {
                    doc::read(&path)
                } else {
                    Ok(Value::Null)
                }
            }
            "status" => Ok(json!({"state":if self.alive{"running"}else{"stopped"}})),
            "predecessor" => {
                if self.failure == Some("predecessor") {
                    return Err("unknown previous".into());
                }
                Ok(
                    json!({"state":"stopped","launcher_alive":false,"tools_sha256":super::super::super::digest::plan_hash(&json!({"wsl":a["effective"]["wsl"],"python":a["effective"]["python"]}))?}),
                )
            }
            "stop" => {
                self.stop_calls += 1;
                self.alive = false;
                Ok(json!({"state":"stopped","runtime":a["runtime"]}))
            }
            "observe" => {
                self.observe_calls += 1;
                self.observed(&a["request"], &a["runtime"])
            }
            "start" => {
                let output = PathBuf::from(a["output"].as_str().unwrap());
                assert!(output.is_dir());
                if self.partial {
                    doc::write_new(
                        &output.join("base.json"),
                        &json!({"state":"not_started","output":output}),
                    )?;
                    return Err("precise partial cleanup".into());
                }
                self.counter += 1;
                self.alive = true;
                self.store.clear();
                let receipt = output.join("fake-process.json");
                doc::write_new(&receipt, &json!({"generation":self.counter}))?;
                let base = json!({"process_receipt":common::binding(&receipt)?,"output":output,"generation":self.counter});
                doc::write_new(&output.join("base.json"), &base)?;
                self.observed(&a["request"], &base)
            }
            "redis" => self.redis(&a),
            _ => Err("unexpected cache port".into()),
        }
    }
}
pub(super) fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let file = entry.unwrap().path();
        if file.is_dir() {
            result.extend(snapshot(&file));
        } else {
            result.insert(file.clone(), fs::read(file).unwrap());
        }
    }
    result
}
pub(super) fn before() -> Value {
    markers::images(&json!({})).0
}

impl Ports {
    fn redis(&mut self, a: &Value) -> Result<Value> {
        if self.failure == Some("scan") {
            return Err("before first SCAN".into());
        }
        let command = &a["parts"];
        self.commands.push(command.clone());
        match command[0].as_str() {
            Some("SCAN") => {
                if self.failure == Some("outside") {
                    return Ok(json!(["0", ["foreign"]]));
                }
                if self.failure == Some("cursor") {
                    return Ok(json!(["3", []]));
                }
                if self.failure == Some("scan-shape") {
                    return Ok(json!(["0", "not-array"]));
                }
                Ok(json!([
                    "0",
                    self.store
                        .keys()
                        .filter(|k| k
                            .starts_with(a["request"]["markers"]["namespace"].as_str().unwrap()))
                        .collect::<Vec<_>>()
                ]))
            }
            Some("GET") => Ok(self
                .store
                .get(command[1].as_str().unwrap())
                .cloned()
                .unwrap_or(Value::Null)),
            Some("MSETNX") => {
                if self.failure == Some("before") {
                    return Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into());
                }
                for index in [1, 3] {
                    if self.store.contains_key(command[index].as_str().unwrap()) {
                        return Ok(json!(0));
                    }
                }
                for index in [1, 3] {
                    self.store.insert(
                        command[index].as_str().unwrap().into(),
                        command[index + 1].clone(),
                    );
                }
                if self.failure == Some("after") {
                    return Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into());
                }
                if self.failure == Some("bool-response") {
                    return Ok(json!(true));
                }
                Ok(json!(1))
            }
            _ => Err("unexpected command".into()),
        }
    }
}
