use super::{dependency::Dependencies, execution, model::*, plan, receipts, tests::budget_value};
use crate::{Result, recovery::clone::native::source};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
struct Fixture {
    _temp: tempfile::TempDir,
    backend: PathBuf,
    frontend: PathBuf,
    runtime: PathBuf,
    bootstrap: PathBuf,
    review: PathBuf,
    tool: PathBuf,
    source: Value,
    server: String,
    frontend_url: String,
    process_state: Value,
    calls: Vec<String>,
    fail: Option<String>,
    drift: bool,
    worker_restarts: usize,
    receipt_tamper: bool,
}
fn put(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}
fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn repo(root: &Path, name: &str) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join(".gitignore"), b".local-tests/\ndist/\n").unwrap();
    fs::write(root.join(name), b"{}").unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
    );
}
impl Fixture {
    fn new(server: &str) -> Self {
        let temp = super::tests::temporary();
        let backend = temp.path().join("backend");
        let frontend = temp.path().join("frontend");
        repo(&backend, "Cargo.toml");
        repo(&frontend, "package.json");
        let runtime = backend.join(".local-tests/reference-fixture/runtime");
        fs::create_dir_all(&runtime).unwrap();
        let environment = backend.join(".local-tests/environment");
        fs::create_dir_all(&environment).unwrap();
        fs::create_dir_all(frontend.join(".local-tests/corepack")).unwrap();
        let tool = backend.join(".local-tests/tools/corepack.cmd");
        fs::create_dir_all(tool.parent().unwrap().join("node_modules/corepack/dist")).unwrap();
        fs::write(&tool, b"tool").unwrap();
        fs::write(
            tool.parent()
                .unwrap()
                .join("node_modules/corepack/dist/corepack.js"),
            b"tool entry",
        )
        .unwrap();
        let secret = environment.join("admin.txt");
        fs::write(&secret, b"PrivateAdminSecret123!").unwrap();
        let mysql_secret = environment.join("mysql.cnf");
        fs::write(&mysql_secret, b"[client]\npassword=DatabaseSecret123!\n").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let frontend_url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let review = backend.join(".local-tests/review.json");
        put(
            &review,
            &json!({"scopes":{"seed":{"scope_id":"fixture-source","api_url":"http://127.0.0.1:19001","frontend_url":frontend_url}},"tools":{"mysql":bound(&tool).unwrap()}}),
        );
        let bootstrap = environment.join("bootstrap.json");
        put(
            &bootstrap,
            &json!({"plan":{"side":"seed","review":bound(&review).unwrap()},"secret_files":{"reset-admin-password.txt":bound(&secret).unwrap(),"mysql-client.cnf":bound(&mysql_secret).unwrap()}}),
        );
        put(
            &environment.join("environment.json"),
            &json!({"APP_ENV":"test"}),
        );
        for file in ["runtime.json", "source-pair.json", "backend-build.json"] {
            put(&runtime.join(file), &json!({"file":file}));
        }
        let mut backend_source =
            source::binding_value(&source::capture(&backend).unwrap().source)["snapshot"].clone();
        backend_source.as_object_mut().unwrap().remove("clean");
        let mut frontend_source =
            source::binding_value(&source::capture(&frontend).unwrap().source)["snapshot"].clone();
        frontend_source.as_object_mut().unwrap().remove("clean");
        let source = json!({"fixture":"business","roots":{"frontend":frontend},"original":{"frontend":frontend_source},"generated":{"backend":backend_source,"frontend":frontend_source}});
        Self {
            _temp: temp,
            backend,
            frontend,
            runtime,
            bootstrap,
            review,
            tool,
            source,
            server: server.into(),
            frontend_url,
            process_state: json!({"api":"running","worker":"running"}),
            calls: vec![],
            fail: None,
            drift: false,
            worker_restarts: 0,
            receipt_tamper: false,
        }
    }
    fn request(&self, operation: &str) -> Value {
        let mut v = json!({"format_version":1,"operation":operation,"backend_dir":self.backend,"environment":self.bootstrap,"output":self.runtime,"browser_binding":self.runtime.join("browser-binding-run-a.json"),"write":(["bind","browser"].contains(&operation))});
        if operation == "bind" {
            v["run_id"] = json!("run-a");
            v["server"] = json!(self.server);
        }
        if operation == "browser" {
            v["credential_endpoint"] = json!("http://127.0.0.1:49152/v1/credentials");
        }
        v
    }
    fn execute(&mut self, operation: &str) -> Result<Value> {
        let request = self.request(operation);
        execution::execute(self, &request)
    }
    fn outputs(&self) -> BTreeMap<String, PathBuf> {
        plan::outputs(&self.frontend, &self.runtime, "run-a", &self.server)
    }
    fn browser_artifacts(&self) {
        let outputs = self.outputs();
        for key in ["report", "results"] {
            fs::create_dir_all(&outputs[key]).unwrap();
        }
        fs::write(outputs["report"].join("index.html"), b"report").unwrap();
        put(
            &outputs["results"].join("business-tests.json"),
            &json!({"format_version":1,"kind":"business-browser-tests","fixture":"business","server":self.server,"run_id":"run-a","status":"passed","runs":receipts::TESTS.iter().map(|(title,scenarios)|json!({"title":[title],"status":"passed","retry":0,"scenarios":scenarios})).collect::<Vec<_>>() }),
        );
        put(&outputs["login_budget"], &budget_value().1);
        if self.server == "preview" {
            let mut entries = vec![];
            for (path, file, destination) in [
                ("/login", "index.html", "document"),
                ("/assets/app.js", "assets/app.js", "script"),
            ] {
                let bound = bound(&self.frontend.join("dist").join(file)).unwrap();
                entries.push(json!({"sequence":entries.len()+1,"method":"GET","path":path,"destination":destination,"status":200,"bytes":bound["bytes"],"sha256":bound["sha256"],"representation":"identity"}));
            }
            put(
                &outputs["response_audit"],
                &json!({"format_version":1,"kind":"business-preview-static-responses","status":"complete","run_id":"run-a","scope_id":"fixture-source","limits":{"entries":MAX_FILES,"bytes":MAX_BYTES},"total_entries":entries.len(),"total_bytes":entries.iter().map(|e|e["bytes"].as_u64().unwrap()).sum::<u64>(),"entries":entries}),
            );
        }
    }
    fn process(&self, directory: &Path) -> Value {
        fs::create_dir_all(directory).unwrap();
        let identity = json!({"pid":123,"started":"1","executable":self.tool});
        let tree = json!({"format_version":2,"kind":"full-stack-process-tree","runtime_directory":directory,"role":"frontend","scope_id":"fixture-source","operation_id":"a".repeat(32),"supervisor":identity,"process":identity,"monitor":{"pid":124,"started":"2","executable":self.tool},"group_id":123});
        put(
            &directory.join("frontend.json"),
            &json!({"format_version":1,"role":"frontend","scope_id":"fixture-source","identity":identity}),
        );
        put(&directory.join("frontend-tree.json"), &tree);
        put(
            &directory.join(format!("frontend-members-{}-stopped.json", "a".repeat(32))),
            &json!({"status":"stopped"}),
        );
        json!({"directory":directory,"process":bound(&directory.join("frontend.json")).unwrap(),"tree":bound(&directory.join("frontend-tree.json")).unwrap(),"completion":bound(&directory.join(format!("frontend-members-{}-stopped.json","a".repeat(32)))).unwrap()})
    }
}
impl Dependencies for Fixture {
    fn call(&mut self, op: &str, args: Value) -> Result<Value> {
        self.calls.push(op.into());
        match op {
            "bootstrap" => Ok(
                json!({"bootstrap":self.bootstrap,"execution":self.backend,"private":{"APP_ENV":"test","APP_PASSWORD":"PrivateIgnoredSecret"},"python":self.tool}),
            ),
            "verify" => Ok(
                json!({"runtime":bound(&self.runtime.join("runtime.json"))?,"build_evidence":{"source":self.source}}),
            ),
            "control" | "observe" => Ok(json!({"processes":self.process_state})),
            "worker" => {
                self.worker_restarts += 1;
                self.process_state["worker"] = json!("running");
                Ok(json!("running"))
            }
            "settings" => Ok(
                json!({"app":{"host":"127.0.0.1","port":19001},"scope_id":"fixture-source","cors":{"allow_origins":[self.frontend_url]}}),
            ),
            "limits" => Ok(
                json!({"capacity":100,"window_secs":1,"api_limits":{"POST /api/v1/auth/login":7},"api_window_secs":30}),
            ),
            "tools" => Ok(
                json!({"corepack":self.tool,"node":self.tool,"corepack_home":self.frontend.join(".local-tests/corepack")}),
            ),
            "frontend-build" => {
                Ok(json!({"path":self.frontend.join("dist/.vite/restore-build.json")}))
            }
            "completion" => {
                let directory = path(&args["directory"])?;
                let (tree, _) = document(&directory.join("frontend-tree.json"))?;
                let proof = directory.join(format!(
                    "frontend-members-{}-stopped.json",
                    text(&tree["operation_id"])?
                ));
                let value =
                    json!({"tree":tree,"completion":bound(&proof)?,"proof":document(&proof)?.0});
                if self.receipt_tamper {
                    fs::write(self.outputs()["report"].join("index.html"), b"tampered")?;
                }
                Ok(value)
            }
            _ => Err("未登记测试依赖操作".into()),
        }
    }
    fn frontend(&mut self, command: super::dependency::FrontendCommand<'_>) -> Result<Value> {
        let super::dependency::FrontendCommand {
            binding,
            arguments,
            environment,
            log,
            directory,
            secrets,
            ..
        } = command;
        let stage = if arguments[0] == "build" {
            "build"
        } else if arguments[0] == "check" {
            "browser"
        } else {
            "verify"
        };
        self.calls.push(format!("frontend:{stage}"));
        assert_eq!(environment["APP_ENV"], "test");
        assert!(environment.get("APP_PASSWORD").is_none());
        assert!(secrets.iter().any(|s| s == "PrivateAdminSecret123!"));
        assert!(secrets.iter().any(|s| s == "DatabaseSecret123!"));
        fs::write(log, b"redacted log")?;
        let evidence = self.process(directory);
        if self.fail.as_deref() == Some(stage) {
            return Err(super::dependency::failure("injected", Some(17)));
        }
        if stage == "build" {
            fs::create_dir_all(self.frontend.join("dist/.vite"))?;
            fs::create_dir(self.frontend.join("dist/assets"))?;
            put(
                &self.frontend.join("dist/.vite/restore-build.json"),
                &json!({"kind":"test"}),
            );
            fs::write(self.frontend.join("dist/index.html"), b"index")?;
            fs::write(self.frontend.join("dist/assets/app.js"), b"script")?;
        }
        if stage == "browser" {
            self.browser_artifacts();
            self.process_state["worker"] = json!("stopped");
            if self.drift {
                fs::write(
                    self.frontend.join(".local-tests/frontend-tools.json"),
                    b"{}",
                )?;
            }
        }
        assert_eq!(binding["scope_id"], "fixture-source");
        Ok(evidence)
    }
}

#[test]
fn preview_build_browser_verify_close_preserve_order_and_bound_origin() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = Fixture::new("preview");
    let binding = f.execute("bind").unwrap();
    assert_eq!(binding["status"], "reference_fixture_browser_bound");
    assert!(f.execute("bind").is_err());
    let result = f.execute("browser").unwrap();
    assert_eq!(result["status"], "passed");
    assert_eq!(f.worker_restarts, 1);
    let stages = f
        .calls
        .iter()
        .filter(|s| s.starts_with("frontend:"))
        .cloned()
        .collect::<Vec<_>>();
    assert!(stages.windows(4).any(|s| s
        == [
            "frontend:build",
            "frontend:verify",
            "frontend:browser",
            "frontend:verify"
        ]));
    let tools = document(&f.frontend.join(".local-tests/frontend-tools.json"))
        .unwrap()
        .0;
    assert_eq!(
        tools["real_browser"]["port"],
        url::Url::parse(&f.frontend_url).unwrap().port().unwrap()
    );
    assert!(!tools.to_string().contains("PrivateAdminSecret"));
    assert!(tools["real_browser"].get("previewResponseAudit").is_some());
    let before = fs::read(f.outputs()["result"].clone()).unwrap();
    f.execute("browser-verify").unwrap();
    assert!(f.execute("browser-close").is_err());
    f.process_state = json!({"api":"stopped","worker":"stopped"});
    assert_eq!(
        f.execute("browser-close").unwrap()["status"],
        "reference_fixture_browser_closed"
    );
    assert_eq!(fs::read(f.outputs()["result"].clone()).unwrap(), before);
}
#[test]
fn dev_uses_independent_run_without_build_and_refuses_extra_build() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = Fixture::new("dev");
    f.execute("bind").unwrap();
    f.execute("browser").unwrap();
    assert!(!f.frontend.join("dist").exists());
    let result_path = f.outputs()["result"].clone();
    let (mut result, _) = document(&result_path).unwrap();
    result["build"] = json!({"unregistered":true});
    put(&result_path, &result);
    assert!(f.execute("browser-verify").is_err());
}
#[test]
fn failure_preserves_exit_unknown_writes_and_blocks_replay() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for (server, stage, unknown) in [("preview", "build", false), ("dev", "browser", true)] {
        let mut f = Fixture::new(server);
        f.execute("bind").unwrap();
        f.fail = Some(stage.into());
        assert!(f.execute("browser").is_err());
        let failure = document(&f.outputs()["failure"]).unwrap().0;
        assert_eq!(failure["returncode"], 17);
        assert_eq!(failure["unknown_business_writes"], unknown);
        assert!(!f.outputs()["result"].exists());
        assert!(f.execute("browser").is_err());
    }
}
#[test]
fn concurrent_tool_drift_and_late_artifact_tamper_fail_before_success() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for mode in ["tools", "artifact"] {
        let mut f = Fixture::new("dev");
        f.execute("bind").unwrap();
        f.drift = mode == "tools";
        f.receipt_tamper = mode == "artifact";
        assert!(f.execute("browser").is_err());
        assert!(f.outputs()["failure"].exists());
        assert!(!f.outputs()["result"].exists());
    }
}
#[test]
fn binding_rejects_unregistered_outputs_review_drift_and_occupied_frontend_port() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = Fixture::new("dev");
    fs::write(f.runtime.join("browser-run-a-unknown.json"), b"{}").unwrap();
    assert!(f.execute("bind").is_err());
    let mut f = Fixture::new("dev");
    f.execute("bind").unwrap();
    fs::write(&f.review, b"{}").unwrap();
    assert!(f.execute("browser").is_err());
    assert!(!f.outputs()["intent"].exists());
    let mut f = Fixture::new("dev");
    let listener =
        std::net::TcpListener::bind(f.frontend_url.strip_prefix("http://").unwrap()).unwrap();
    assert!(f.execute("bind").is_err());
    drop(listener);
}
#[test]
fn source_guard_tolerates_identical_touch_but_detects_content_and_identity_change() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let f = Fixture::new("dev");
    let root = &f.backend;
    let expected = &f.source["generated"]["backend"];
    let guard = plan::SourceGuard::capture(root, expected).unwrap();
    let file = fs::File::options()
        .write(true)
        .open(root.join("Cargo.toml"))
        .unwrap();
    file.set_times(
        fs::FileTimes::new()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2)),
    )
    .unwrap();
    guard.unchanged().unwrap();
    fs::write(root.join("Cargo.toml"), b"changed").unwrap();
    assert!(guard.unchanged().is_err());
}

#[test]
fn intent_failure_records_absent_or_visible_intent_before_business_writes() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for visible in [false, true] {
        let mut f = Fixture::new("dev");
        f.execute("bind").unwrap();
        let request = f.request("browser");
        let paths = plan::Paths {
            backend: f.backend.clone(),
            environment: f.bootstrap.clone(),
            output: f.runtime.clone(),
            binding: path(&request["browser_binding"]).unwrap(),
        };
        let context = plan::make(&mut f, &paths, "run-a", "dev", true, false, false).unwrap();
        let guard = snapshot(&paths.binding, 1024 * 1024).unwrap();
        assert!(
            execution::run_with(
                &mut f,
                &paths,
                context,
                guard,
                "http://127.0.0.1:49152/v1/credentials",
                |path, value| {
                    if visible {
                        write_new(path, value)?;
                    }
                    Err("injected".into())
                }
            )
            .is_err()
        );
        let failure = document(&f.outputs()["failure"]).unwrap().0;
        assert_eq!(failure["stage"], "intent");
        assert_eq!(failure["unknown_business_writes"], false);
        assert_eq!(
            failure["intent"]["status"],
            if visible { "published" } else { "absent" }
        );
        assert!(!f.outputs()["result"].exists());
    }
}
#[test]
fn consumer_rechecks_all_artifacts_logs_processes_and_unknown_terminal_commit() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = Fixture::new("preview");
    f.execute("bind").unwrap();
    f.execute("browser").unwrap();
    let outputs = f.outputs();
    let original = document(&outputs["result"]).unwrap().0;
    let report = outputs["report"].join("index.html");
    let report_bytes = fs::read(&report).unwrap();
    fs::write(&report, b"changed").unwrap();
    assert!(f.execute("browser-verify").is_err());
    fs::write(&report, report_bytes).unwrap();
    let original_log = fs::read(&outputs["browser_log"]).unwrap();
    fs::write(&outputs["browser_log"], b"PrivateAdminSecret123!").unwrap();
    let mut changed = original.clone();
    changed["logs"]["browser"] = bound(&outputs["browser_log"]).unwrap();
    put(&outputs["result"], &changed);
    assert!(f.execute("browser-verify").is_err());
    fs::write(&outputs["browser_log"], original_log).unwrap();
    put(&outputs["result"], &original);
    let process_path = outputs["browser_process"].join("frontend.json");
    let (mut process, _) = document(&process_path).unwrap();
    let original_process = process.clone();
    process["identity"]["pid"] = json!(999);
    put(&process_path, &process);
    let mut changed = original.clone();
    changed["processes"]["browser"]["process"] = bound(&process_path).unwrap();
    put(&outputs["result"], &changed);
    assert!(f.execute("browser-verify").is_err());
    put(&process_path, &original_process);
    put(&outputs["result"], &original);
    fs::write(
        f.runtime
            .join("browser-run-a-result.json.pending-interrupted"),
        b"unknown",
    )
    .unwrap();
    assert!(f.execute("browser-verify").is_err());
}
#[test]
fn failure_evidence_collects_completed_and_partial_receipts_with_exact_names() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = Fixture::new("preview");
    f.execute("bind").unwrap();
    f.fail = Some("browser".into());
    assert!(f.execute("browser").is_err());
    let failure = document(&f.outputs()["failure"]).unwrap().0;
    for name in ["build", "build_verify_before", "browser"] {
        assert!(failure["processes"].get(name).is_some());
        assert!(failure["logs"].get(name).is_some());
    }
    let directory = f.runtime.join("partial-process");
    fs::create_dir(&directory).unwrap();
    put(
        &directory.join("frontend-tree.json"),
        &json!({"operation_id":"a".repeat(32)}),
    );
    for name in [
        format!("frontend-tree-{}-result.json", "a".repeat(32)),
        format!("frontend-members-{}-stopped.json", "a".repeat(32)),
    ] {
        put(&directory.join(name), &json!({"partial":true}));
    }
    let partial = super::process::partial(&mut f, &directory, "fixture-source").unwrap();
    assert!(partial.get("tree_result").is_some());
    assert!(partial.get("completion").is_some());
}
#[test]
fn worker_recovery_keeps_running_worker_and_rejects_unknown_or_missing_api() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = Fixture::new("dev");
    let paths = plan::Paths {
        backend: f.backend.clone(),
        environment: f.bootstrap.clone(),
        output: f.runtime.clone(),
        binding: f.runtime.join("browser-binding-run-a.json"),
    };
    execution::ensure_worker(&mut f, &paths).unwrap();
    assert_eq!(f.worker_restarts, 0);
    f.process_state["worker"] = json!("unknown");
    assert!(execution::ensure_worker(&mut f, &paths).is_err());
    f.process_state = json!({"api":"stopped","worker":"stopped"});
    assert!(execution::ensure_worker(&mut f, &paths).is_err());
}
