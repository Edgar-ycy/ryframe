use super::*;
pub(super) struct Fixture {
    pub(super) _temporary: tempfile::TempDir,
    pub(super) backend: PathBuf,
    pub(super) run: PathBuf,
    pub(super) review: PathBuf,
    pub(super) bootstrap: PathBuf,
    pub(super) services: Value,
}
impl Fixture {
    pub(super) fn new() -> Self {
        let backend = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        fs::create_dir_all(backend.join(".local-tests")).unwrap();
        let temporary = tempfile::tempdir_in(backend.join(".local-tests")).unwrap();
        let root =
            super::super::super::super::portable_path(&fs::canonicalize(temporary.path()).unwrap());
        let run = root.join("service-run");
        fs::create_dir(&run).unwrap();
        let review = root.join("review.json");
        let bootstrap = root.join("bootstrap.json");
        json_io::write_new(&review, &json!({"fixture":true})).unwrap();
        json_io::write_new(&bootstrap, &json!({"fixture":true})).unwrap();
        json_io::write_new(
            &root.join("environment.json"),
            &json!({"environment":{"APP_ENV":"test"}}),
        )
        .unwrap();
        let manifest = json!({"format_version":1,"kind":"reference-fixture-service-run","review":binding(&review).unwrap(),
            "bootstrap":binding(&bootstrap).unwrap(),"execution_backend":backend,"scope_id":"fixture-service-test","data_directory_was_empty":true});
        json_io::write_new(&run.join("manifest.json"), &manifest).unwrap();
        state::initialize(&run).unwrap();
        for role in ["rustfs", "redis"] {
            fs::create_dir(run.join(role)).unwrap();
        }
        let identity = json!({"pid":2147481000u32,"started":"1","executable":"fixture.exe"});
        let tree = json!({"format_version":2,"runtime_directory":run.join("rustfs"),"role":"rustfs","scope_id":"fixture-service-test",
            "operation_id":"b".repeat(32),"supervisor":identity,"monitor":{"pid":2147481001u32,"started":"1","executable":"fixture.exe"},
            "process":{"pid":2147481002u32,"started":"1","executable":"fixture.exe"}});
        json_io::write_new(&run.join("rustfs").join("rustfs-tree.json"), &tree).unwrap();
        let runtime = json!({"output":run.join("redis"),"linux_identity":{"pid":201,"started":"11","executable":"/redis","boot_id":"boot-1"},"redis":{"run_id":"1".repeat(40)}});
        json_io::write_new(&run.join("redis").join("runtime.json"), &runtime).unwrap();
        let requests = json!({"rustfs":{"scope_id":"fixture-service-test","executable":{"path":"rustfs.exe","sha256":"a".repeat(64)},"data_directory":{"path":run.join("rustfs")}},
            "redis":{"configuration":{"path":run.join("redis.conf")},"port":16390,"previous_identity":null,"previous_boot_id":null,"previous_run_id":null}});
        for role in ["rustfs", "redis"] {
            json_io::write_new(&run.join(role).join("request.json"), &requests[role]).unwrap();
        }
        for (stage, mode) in [
            ("storage-target", "initial"),
            ("cache-target", "initial"),
            ("fixture-buckets", "prepare"),
        ] {
            let (number, _) = state::begin(&run, stage, mode, &json!({})).unwrap();
            state::finish(&run, number, Some(&json!({"status":stage})), None).unwrap();
        }
        let services = json!({"tree":tree,"requests":requests,"runtime":runtime,"storage":{},"origin":{"storage":{},"redis":{}},
            "evidence":{"rustfs_tree":binding(&run.join("rustfs").join("rustfs-tree.json")).unwrap()}});
        Self {
            _temporary: temporary,
            backend,
            run,
            review,
            bootstrap,
            services,
        }
    }
    pub(super) fn state(&self) -> Value {
        state::load(&self.run, true).unwrap()
    }
    pub(super) fn history(&self) -> Value {
        history::validate(&self.run, &self.state(), true, true).unwrap()
    }
    pub(super) fn context(&self) -> Value {
        let manifest = read(&self.run.join("manifest.json")).unwrap();
        json!({"backend":self.backend,"run":self.run,"manifest":manifest,"state":self.state(),
            "sources":{"review":binding(&self.review).unwrap(),"bootstrap":binding(&self.bootstrap).unwrap(),"manifest":binding(&self.run.join("manifest.json")).unwrap(),
                "state_before":binding(&self.run.join("state.json")).unwrap(),"environment":binding(&self.bootstrap.parent().unwrap().join("environment.json")).unwrap()},
            "private":{"APP_ENV":"test"},"documents":{"review":read(&self.review).unwrap(),"bootstrap":read(&self.bootstrap).unwrap(),"manifest":manifest}})
    }
}

pub(super) struct FakeRuntime {
    pub(super) fixture: Fixture,
    pub(super) events: Vec<String>,
    pub(super) fail: Option<String>,
    pub(super) drift_on_stop: bool,
    pub(super) controller: Value,
    pub(super) observation: Value,
    pub(super) new_running: bool,
    pub(super) identities: VecDeque<&'static str>,
    pub(super) clock: Duration,
    pub(super) pauses: usize,
    pub(super) drift_observe_at: Option<usize>,
    pub(super) observes: usize,
}
impl FakeRuntime {
    pub(super) fn new() -> Self {
        Self {
            fixture: Fixture::new(),
            events: Vec::new(),
            fail: None,
            drift_on_stop: false,
            controller: Value::Null,
            observation: expected_running(),
            new_running: false,
            identities: VecDeque::new(),
            clock: Duration::ZERO,
            pauses: 0,
            drift_observe_at: None,
            observes: 0,
        }
    }
    pub(super) fn execute(&mut self, operation: &str, owner: Option<PathBuf>) -> Result<Value> {
        let (backend, review, bootstrap) = (
            self.fixture.backend.clone(),
            self.fixture.review.clone(),
            self.fixture.bootstrap.clone(),
        );
        execute_with(
            self,
            &backend,
            operation,
            &review,
            &bootstrap,
            owner.as_deref(),
            operation != "status",
        )
    }
    pub(super) fn owner(&self) -> PathBuf {
        self.fixture.run.join("state.json")
    }
    pub(super) fn event_position(&self, event: &str) -> usize {
        self.events.iter().position(|item| item == event).unwrap()
    }
    pub(super) fn external(&mut self) {
        let tree = &self.fixture.services["tree"];
        let ready = self
            .fixture
            .run
            .join("rustfs")
            .join(format!("rustfs-members-{}-ready.json", "b".repeat(32)));
        json_io::write_new(&ready, &json!({"ready":true})).unwrap();
        self.observation = json!({"redis":"stopped","rustfs":"external-termination-unreconciled","termination":{"status":"external-termination-unreconciled",
            "identities":{"supervisor":"missing","monitor":"missing","process":"missing"},"normal_shutdown_proof":null,
            "evidence":{"rustfs_tree":binding(Path::new(text(tree,"runtime_directory").unwrap()).join("rustfs-tree.json").as_path()).unwrap(),
            "redis_runtime":binding(&self.fixture.run.join("redis").join("runtime.json")).unwrap(),"rustfs_monitor_ready":binding(&ready).unwrap()}}});
    }
    pub(super) fn stopped_members(&self, tree: &Value) -> Value {
        let path = Path::new(text(tree, "runtime_directory").unwrap()).join(format!(
            "rustfs-members-{}-stopped.json",
            text(tree, "operation_id").unwrap()
        ));
        if !path.exists() {
            json_io::write_new(&path,&json!({"directory":tree["runtime_directory"],"status":"stopped","error_type":null,
                "operation_id":tree["operation_id"],"role":tree["role"],"scope_id":tree["scope_id"],"monitor":tree["monitor"],
                "supervisor":tree["supervisor"],"members":[tree["supervisor"],tree["monitor"],tree["process"]]})).unwrap();
        }
        binding(&path).unwrap()
    }
}

impl Runtime for FakeRuntime {
    fn now(&self) -> Duration {
        self.clock
    }
    fn pause(&mut self, delay: Duration) {
        self.clock += delay;
        self.pauses += 1;
    }
    fn call(&mut self, operation: &str, args: Value) -> Result<Value> {
        self.events.push(operation.to_owned());
        if self.fail.as_deref() == Some(operation) {
            return Err(model::error("注入失败，不包含私有配置"));
        }
        match operation {
            "context" => Ok(self.fixture.context()),
            "registered" => Ok(self.fixture.services.clone()),
            "review-successor" => Ok(Value::Null),
            "controller" => Ok(self.controller.clone()),
            "observe" => {
                self.observes += 1;
                if self.drift_observe_at == Some(self.observes) {
                    return Ok(expected_running());
                }
                if args["services"]["tree"]["operation_id"] == "c".repeat(32) {
                    Ok(if self.new_running {
                        expected_running()
                    } else {
                        expected_stopped()
                    })
                } else {
                    Ok(self.observation.clone())
                }
            }
            "identity" => match self.identities.pop_front().unwrap_or("missing") {
                "denied" => Err(Box::new(model::LifecycleError {
                    category: "PermissionError",
                    message: "身份暂不可读",
                })),
                "other-error" => Err(model::error("身份读取其他错误")),
                "reused" => Ok(json!({"pid":args["pid"],"started":"reused","executable":"other"})),
                "alive" => Ok(self.fixture.services["tree"]["supervisor"].clone()),
                _ => Ok(Value::Null),
            },
            "cache-stop" => {
                let value = json!({"status":"redis_process_stopped","alive":false,"resources_deleted":false,"runtime":args["runtime"]});
                json_io::write_new(
                    &Path::new(text(&args, "output")?).join("stopped.json"),
                    &value,
                )?;
                self.observation["redis"] = json!("stopped");
                if self.drift_on_stop {
                    fs::write(&self.fixture.review, b"{}").unwrap();
                }
                Ok(value)
            }
            "tree-stop" => {
                if args["crash"] == true {
                    self.new_running = false;
                } else {
                    self.observation = expected_stopped();
                    if args["tree"]["operation_id"] == "c".repeat(32) {
                        self.new_running = false;
                    }
                }
                Ok(json!(true))
            }
            "completion" => Ok(self.stopped_members(&args["tree"])),
            "tree-read" => read(&Path::new(text(&args, "output")?).join("rustfs-tree.json")),
            "storage-start" => start_storage(self, &args),
            "cache-start" => start_cache(&args),
            "recover-controller" => recover_controller(self, &args),
            _ => Err(model::error("未登记测试原语")),
        }
    }
}

fn start_storage(runtime: &mut FakeRuntime, args: &Value) -> Result<Value> {
    let output = Path::new(text(args, "output")?);
    let mut tree = runtime.fixture.services["tree"].clone();
    tree["runtime_directory"] = json!(output);
    tree["operation_id"] = json!("c".repeat(32));
    tree["process"]["started"] = json!("new");
    for (filename, value) in [
        ("process.json", json!({"identity":tree["process"]})),
        ("launch.json", json!({"started":true})),
        ("rustfs-tree.json", tree.clone()),
    ] {
        json_io::write_new(&output.join(filename), &value)?;
    }
    runtime.new_running = true;
    Ok(
        json!({"identity":tree["process"],"sha256":args["request"]["executable"]["sha256"],"process_receipt":binding(&output.join("process.json"))?,
        "launch_receipt":binding(&output.join("launch.json"))?,"tree":binding(&output.join("rustfs-tree.json"))?}),
    )
}
fn start_cache(args: &Value) -> Result<Value> {
    let output = Path::new(text(args, "output")?);
    let value = json!({"output":output,"linux_identity":{"pid":301,"started":"22","executable":"/redis","boot_id":"boot-2"},"redis":{"run_id":"2".repeat(40)}});
    json_io::write_new(&output.join("runtime.json"), &value)?;
    Ok(value)
}
fn recover_controller(runtime: &mut FakeRuntime, args: &Value) -> Result<Value> {
    let run = &runtime.fixture.run;
    let mut value = runtime.fixture.state();
    for item in value["attempts"].as_array_mut().unwrap() {
        if item["status"] == "running" {
            item["status"] = json!("failed");
            item["error_type"] = json!("ControllerInterrupted");
            item["finished_at"] = json!("finished");
        }
    }
    let proof = json!({"status":"controller_recovered","owner":args["owner"],"remote_writes":0});
    json_io::write_new(
        &run.join("recovery-test.intent.json"),
        &json!({"before":binding(&run.join("state.json"))?}),
    )?;
    json_io::write_replace(&run.join("state.json"), &value)?;
    if run.join("run.lock").exists() {
        fs::remove_file(run.join("run.lock/owner.json"))?;
        fs::remove_dir(run.join("run.lock"))?;
    }
    json_io::write_new(&run.join("recovered-test.json"), &proof)?;
    runtime.controller = Value::Null;
    Ok(proof)
}
