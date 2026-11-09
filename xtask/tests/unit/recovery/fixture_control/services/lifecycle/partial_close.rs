use super::status_tests::snapshot;
use super::*;

struct PartialRuntime {
    inner: FakeRuntime,
    stopped: bool,
    changed_identity: bool,
    lose_stop_response: bool,
    late_initial_evidence: bool,
    cache_state: &'static str,
}

impl PartialRuntime {
    fn new() -> Self {
        Self::with_cache(false)
    }

    fn with_cache(passed: bool) -> Self {
        let inner = registration_tests::frozen();
        let mut ledger = inner.fixture.state();
        fs::remove_file(Path::new(
            ledger["attempts"][2]["result"]["path"].as_str().unwrap(),
        ))
        .unwrap();
        ledger["attempts"].as_array_mut().unwrap().truncate(2);
        let result = PathBuf::from(ledger["attempts"][1]["result"]["path"].as_str().unwrap());
        let cache = read(&result).unwrap();
        fs::remove_file(result).unwrap();
        ledger["attempts"][1]["status"] = json!("running");
        ledger["attempts"][1]["result"] = Value::Null;
        ledger["attempts"][1]["finished_at"] = Value::Null;
        ledger["attempts"][1]["error_type"] = Value::Null;
        let manifest = read(&inner.fixture.run.join("manifest.json")).unwrap();
        for attempt in ledger["attempts"].as_array_mut().unwrap() {
            attempt["sources"]["review"] = manifest["review"].clone();
            attempt["sources"]["bootstrap"] = manifest["bootstrap"].clone();
        }
        let output = inner.fixture.run.join("rustfs");
        let request = output.join("request.json");
        // frozen 使用同一 scope 构造意图、启动和进程；此处只补清单的匹配事实。
        let manifest_file = inner.fixture.run.join("manifest.json");
        let mut manifest = manifest;
        manifest["scope_id"] = read(&request).unwrap()["scope_id"].clone();
        json_io::write_replace(&manifest_file, &manifest).unwrap();
        ledger["manifest"] = binding(&manifest_file).unwrap();
        let tree_file = output.join("rustfs-tree.json");
        let tree = initial_tree(&output, &manifest);
        json_io::write_replace(&tree_file, &tree).unwrap();
        let intent_file = output.join("intent.json");
        let mut intent = read(&intent_file).unwrap();
        intent["request"] = binding(&manifest_file).unwrap();
        json_io::write_replace(&intent_file, &intent).unwrap();
        let process_file = output.join("process.json");
        let mut process = read(&process_file).unwrap();
        process["intent"] = binding(&intent_file).unwrap();
        json_io::write_replace(&process_file, &process).unwrap();
        let result_file = PathBuf::from(ledger["attempts"][0]["result"]["path"].as_str().unwrap());
        let mut result = read(&result_file).unwrap();
        result["process_receipt"] = binding(&process_file).unwrap();
        result["tree"] = binding(&tree_file).unwrap();
        json_io::write_replace(&result_file, &result).unwrap();
        ledger["attempts"][0]["result"] = binding(&result_file).unwrap();
        json_io::write_replace(&inner.fixture.run.join("state.json"), &ledger).unwrap();
        let owner = RunLock::claim(&inner.fixture.run).unwrap();
        state::bind_controller(&inner.fixture.run, 2, &owner.owner, &ledger["attempts"][1])
            .unwrap();
        state::finish(
            &inner.fixture.run,
            2,
            passed.then_some(&cache),
            (!passed).then_some("ValueError"),
        )
        .unwrap();
        owner.release().unwrap();
        if !passed {
            fs::remove_file(inner.fixture.run.join("redis").join("runtime.json")).unwrap();
        }
        Self {
            inner,
            stopped: false,
            changed_identity: false,
            lose_stop_response: false,
            late_initial_evidence: false,
            cache_state: if passed { "running" } else { "stopped" },
        }
    }

    fn execute(&mut self, operation: &str) -> Result<Value> {
        let f = &self.inner.fixture;
        let (backend, review, bootstrap) =
            (f.backend.clone(), f.review.clone(), f.bootstrap.clone());
        execute_with(
            self,
            &backend,
            operation,
            &review,
            &bootstrap,
            None,
            operation != "status",
        )
    }
}

fn initial_tree(output: &Path, manifest: &Value) -> Value {
    let mut tree = read(&output.join("rustfs-tree.json")).unwrap();
    tree["kind"] = json!("full-stack-process-tree");
    tree["scope_id"] = manifest["scope_id"].clone();
    tree["runtime_directory"] = json!(output);
    for role in ["supervisor", "monitor"] {
        tree[role]["executable"] = tree["process"]["executable"].clone();
    }
    for role in ["supervisor", "monitor", "process"] {
        crate::recovery::clone::native::resources::common::validate_process_identity(&tree[role])
            .unwrap();
    }
    assert_ne!(tree["supervisor"]["pid"], tree["monitor"]["pid"]);
    assert_ne!(tree["supervisor"]["pid"], tree["process"]["pid"]);
    assert_ne!(tree["monitor"]["pid"], tree["process"]["pid"]);
    tree["group_id"] = tree["supervisor"]["pid"].clone();
    let ready = json!({"format_version":1,"operation_id":tree["operation_id"],
        "directory":output,"role":tree["role"],"scope_id":tree["scope_id"],
        "supervisor":tree["supervisor"],"monitor":tree["monitor"]});
    json_io::write_new(
        &output.join(format!(
            "rustfs-members-{}-ready.json",
            text(&tree, "operation_id").unwrap()
        )),
        &ready,
    )
    .unwrap();
    tree
}

fn stopped_tree(tree: &Value) -> Value {
    json!({"format_version":1,"operation_id":tree["operation_id"],
        "directory":tree["runtime_directory"],"role":tree["role"],"scope_id":tree["scope_id"],
        "supervisor":tree["supervisor"],"monitor":tree["monitor"],
        "status":"stopped","members":[tree["process"],tree["supervisor"]],"error_type":null})
}

fn completion(tree: &Value, run: &Path) -> Result<Value> {
    let path = observation::receipt(tree, "stopped")?;
    let descriptor = binding(&path)?;
    check(
        read(&path)? == stopped_tree(tree),
        "夹具完整成员收据与原树不同",
    )?;
    history::evidence::members(
        &json!({"state":"stopped","tree":tree,"completion":descriptor}),
        run,
    )?;
    Ok(descriptor)
}

impl Runtime for PartialRuntime {
    fn now(&self) -> Duration {
        self.inner.now()
    }
    fn pause(&mut self, delay: Duration) {
        self.inner.pause(delay);
    }
    fn call(&mut self, operation: &str, args: Value) -> Result<Value> {
        if operation == "cache-status" {
            self.inner.events.push(operation.to_owned());
            check(
                self.inner.fail.as_deref() != Some(operation),
                "缓存身份或端口核验失败",
            )?;
            return Ok(json!({"state":self.cache_state}));
        }
        if operation == "tree-read" {
            match self.inner.fail.as_deref() {
                Some("tree-read") => {
                    return Err(
                        std::io::Error::other("synthetic-private-json credential-marker").into(),
                    );
                }
                Some("tree-read-rejected") => {
                    return super::super::super::storage::check(
                        false,
                        "RustFS 输入字节或内核文件身份变化",
                    )
                    .map(|()| Value::Null);
                }
                Some("tree-read-unknown") => {
                    return Err("synthetic-private-json credential-marker".into());
                }
                _ => {}
            }
        }
        if operation == "identity" {
            if self.stopped {
                return Ok(Value::Null);
            }
            let tree = read(
                &self
                    .inner
                    .fixture
                    .run
                    .join("rustfs")
                    .join("rustfs-tree.json"),
            )?;
            let mut identity = ["supervisor", "monitor", "process"]
                .iter()
                .map(|role| &tree[*role])
                .find(|identity| identity["pid"] == args["pid"])
                .unwrap()
                .clone();
            if self.changed_identity {
                identity["started"] = json!("2");
            }
            return Ok(identity);
        }
        if operation == "storage-running" {
            return Ok(json!(!self.stopped));
        }
        if operation == "members-wait" || operation == "completion" {
            check(self.stopped, "夹具监督树尚未停止")?;
            let descriptor = completion(&args["tree"], &self.inner.fixture.run)?;
            return Ok(if operation == "completion" {
                descriptor
            } else {
                Value::Null
            });
        }
        let tree = args["tree"].clone();
        let result = self.inner.call(operation, args);
        if operation == "cache-stop" && result.is_ok() {
            self.cache_state = "stopped";
        }
        if operation == "tree-stop" && result.is_ok() {
            self.stopped = true;
            json_io::write_new(
                &observation::receipt(&tree, "stopped")?,
                &stopped_tree(&tree),
            )?;
            if self.late_initial_evidence {
                fs::write(
                    self.inner.fixture.run.join("redis").join("launch.json"),
                    b"{}",
                )
                .unwrap();
            }
            if self.lose_stop_response {
                return Err(model::error("停止结果未知"));
            }
        }
        result
    }
}

#[test]
fn passed_initial_cache_state_controls_bucket_advice_and_single_partial_close() {
    for cache in ["running", "stopped"] {
        let mut runtime = PartialRuntime::with_cache(true);
        runtime.cache_state = cache;
        let prefix = runtime.inner.fixture.state()["attempts"].clone();
        let before = snapshot(&runtime.inner.fixture.run);
        let tree = read(&runtime.inner.fixture.run.join("rustfs/rustfs-tree.json")).unwrap();
        let status = runtime.execute("status").unwrap();
        assert_eq!(
            status["next_operation"],
            if cache == "running" {
                "buckets"
            } else {
                "close"
            }
        );
        assert_eq!(snapshot(&runtime.inner.fixture.run), before);
        assert!(
            runtime
                .inner
                .events
                .iter()
                .all(|event| !event.ends_with("-stop"))
        );
        let result = runtime.execute("close").unwrap();
        assert_eq!(result["status"], "services_partially_closed");
        assert_eq!(
            &runtime.inner.fixture.state()["attempts"]
                .as_array()
                .unwrap()[..2],
            prefix.as_array().unwrap().as_slice()
        );
        let output = runtime.inner.fixture.run.join("lifecycle-0003");
        let stopped = read(&output.join("stopped.json")).unwrap();
        assert_eq!(
            stopped["runtime"],
            read(&runtime.inner.fixture.run.join("redis/runtime.json")).unwrap()
        );
        assert_eq!(stopped["alive"], false);
        assert_eq!(stopped["resources_deleted"], false);
        assert_eq!(stopped["already_stopped"] == true, cache == "stopped");
        assert_eq!(
            runtime
                .inner
                .events
                .iter()
                .filter(|event| *event == "cache-stop")
                .count(),
            usize::from(cache == "running")
        );
        assert_eq!(
            runtime
                .inner
                .events
                .iter()
                .filter(|event| *event == "tree-stop")
                .count(),
            1
        );
        let proof = read(&output.join("partial-closed.json")).unwrap();
        assert_eq!(proof["rustfs"]["tree"], tree);
        assert_eq!(
            read(Path::new(
                text(&proof["rustfs"]["completion"], "path").unwrap()
            ))
            .unwrap(),
            stopped_tree(&tree)
        );
        let ledger = runtime.inner.fixture.state();
        assert_eq!(
            history::validate(&runtime.inner.fixture.run, &ledger, false, true).unwrap()["closed"],
            true
        );
        assert_eq!(
            runtime.execute("status").unwrap()["next_operation"],
            "new-scenario"
        );
        let controls = runtime
            .inner
            .events
            .iter()
            .filter(|event| event.ends_with("-stop"))
            .count();
        assert_eq!(
            runtime.execute("close").unwrap()["status"],
            "services_already_partially_closed"
        );
        assert_eq!(runtime.inner.fixture.state(), ledger);
        assert_eq!(
            runtime
                .inner
                .events
                .iter()
                .filter(|event| event.ends_with("-stop"))
                .count(),
            controls
        );
    }
}

#[test]
fn passed_initial_unknown_cache_or_changed_inputs_reject_before_any_control() {
    for case in ["unknown", "permission", "reused", "drift"] {
        let mut runtime = PartialRuntime::with_cache(true);
        runtime.cache_state = "stopped";
        match case {
            "unknown" => runtime.cache_state = "unknown",
            "permission" => runtime.inner.fail = Some("cache-status".into()),
            "reused" => runtime.changed_identity = true,
            "drift" => {
                fs::write(runtime.inner.fixture.run.join("redis/runtime.json"), b"{}").unwrap()
            }
            _ => unreachable!(),
        }
        let before = snapshot(&runtime.inner.fixture.run);
        let status = runtime.execute("status").unwrap();
        assert_eq!(status["next_operation"], "reconcile-evidence", "{case}");
        assert!(status["partial_close_rejection"].is_object());
        assert!(runtime.execute("close").is_err(), "{case}");
        assert_eq!(snapshot(&runtime.inner.fixture.run), before);
        assert!(!runtime.stopped);
        assert!(
            runtime
                .inner
                .events
                .iter()
                .all(|event| !event.ends_with("-stop") && !event.ends_with("-start"))
        );
    }
}

#[test]
fn partial_initial_close_preserves_failed_prefix_and_never_becomes_restartable() {
    let mut runtime = PartialRuntime::new();
    let prefix = state::attempts(&runtime.inner.fixture.state())
        .unwrap()
        .to_vec();
    let request = fs::read(runtime.inner.fixture.run.join("redis").join("request.json")).unwrap();
    let before = fs::read(runtime.inner.fixture.run.join("state.json")).unwrap();
    let status = runtime.execute("status").unwrap();
    if status["next_operation"] != "close" {
        match runtime.execute("close") {
            Err(error) => panic!("部分关闭 status 未返回 close；正式 close 原始拒绝原因：{error}"),
            Ok(value) => panic!(
                "部分关闭 status 与正式 close 结果不一致：{}",
                value["status"]
            ),
        }
    }
    assert_eq!(status["next_operation"], "close");
    assert!(status.get("partial_close_rejection").is_none());
    assert_eq!(
        fs::read(runtime.inner.fixture.run.join("state.json")).unwrap(),
        before
    );
    let result = runtime.execute("close").unwrap();
    assert_eq!(result["status"], "services_partially_closed");
    assert_eq!(
        result["services"],
        json!({"rustfs":"stopped","redis":"not_started"})
    );
    let ledger = runtime.inner.fixture.state();
    assert_eq!(state::attempts(&ledger).unwrap()[..2], prefix);
    let history = history::validate(&runtime.inner.fixture.run, &ledger, false, true).unwrap();
    assert_eq!(history["closed"], true);
    assert_eq!(history["initial_complete"], false);
    assert!(history["restart_predecessor"].is_null());
    assert_eq!(
        runtime.execute("status").unwrap()["next_operation"],
        "new-scenario"
    );
    let before = fs::read(runtime.inner.fixture.run.join("state.json")).unwrap();
    assert_eq!(
        runtime.execute("close").unwrap()["status"],
        "services_already_partially_closed"
    );
    assert_eq!(
        fs::read(runtime.inner.fixture.run.join("state.json")).unwrap(),
        before
    );
    assert_eq!(
        fs::read(runtime.inner.fixture.run.join("redis").join("request.json")).unwrap(),
        request
    );
    assert!(
        !runtime
            .inner
            .events
            .iter()
            .any(|event| event == "cache-stop" || event.ends_with("-start"))
    );
}

#[test]
fn partial_close_rejects_unknown_initial_writes_and_foreign_identity_before_stopping() {
    for unknown in [
        "intent.json",
        "launch.json",
        "process.json",
        "runtime.json",
        "unknown",
    ] {
        let mut runtime = PartialRuntime::new();
        fs::write(runtime.inner.fixture.run.join("redis").join(unknown), b"{}").unwrap();
        let before = runtime.inner.fixture.state();
        let status = runtime.execute("status").unwrap();
        assert_eq!(status["next_operation"], "reconcile-evidence");
        assert_eq!(
            status["partial_close_rejection"]["category"],
            "LifecycleRejected"
        );
        assert!(
            status["partial_close_rejection"]["reason"]
                .as_str()
                .is_some_and(|reason| !reason.trim().is_empty())
        );
        assert!(status["partial_close_rejection"]["advice"].is_string());
        assert!(runtime.execute("close").is_err(), "{unknown}");
        assert_eq!(runtime.inner.fixture.state(), before);
        assert!(!runtime.stopped);
    }
    let mut runtime = PartialRuntime::new();
    runtime.changed_identity = true;
    assert!(runtime.execute("close").is_err());
    assert!(!runtime.stopped);
    for relative in [
        "rustfs/process.json",
        "rustfs/rustfs-tree.json",
        "results/unknown.json",
    ] {
        let mut runtime = PartialRuntime::new();
        fs::write(runtime.inner.fixture.run.join(relative), b"{}").unwrap();
        let before = runtime.inner.fixture.state();
        assert!(runtime.execute("close").is_err(), "{relative}");
        assert_eq!(runtime.inner.fixture.state(), before);
        assert!(!runtime.stopped);
    }
    for (failure, reason) in [
        ("tree-read", "I/O kind=Other, os_code=None"),
        ("tree-read-rejected", "RustFS 输入字节或内核文件身份变化"),
        (
            "tree-read-unknown",
            "部分关闭资格核验未完成，原始证据、文件或进程身份无法安全确认",
        ),
    ] {
        let mut runtime = PartialRuntime::new();
        runtime.inner.fail = Some(failure.to_owned());
        let before = snapshot(&runtime.inner.fixture.run);
        let status = runtime.execute("status").unwrap();
        assert_eq!(status["next_operation"], "reconcile-evidence");
        assert_eq!(
            status["partial_close_rejection"],
            json!({
            "category":"LifecycleFailure","reason":reason,
            "advice":"核对原始首代账本、冻结请求、控制器、启动收据与完整进程树；保留现场，不重签绑定、不重放启动，不绕过状态执行关闭"})
        );
        assert!(!status.to_string().contains("credential-marker"));
        assert!(!status.to_string().contains("synthetic-private-json"));
        if failure == "tree-read-rejected" {
            assert_eq!(runtime.execute("close").unwrap_err().to_string(), reason);
        }
        assert_eq!(snapshot(&runtime.inner.fixture.run), before);
        assert!(!runtime.stopped);
        assert!(
            !runtime
                .inner
                .events
                .iter()
                .any(|event| { event.ends_with("-stop") || event.ends_with("-start") })
        );
    }
}

#[test]
fn new_initial_evidence_during_partial_stop_prevents_success_and_is_preserved() {
    let mut runtime = PartialRuntime::new();
    runtime.late_initial_evidence = true;
    assert!(runtime.execute("close").is_err());
    assert!(runtime.stopped);
    assert_eq!(
        runtime.inner.fixture.state()["attempts"][2]["status"],
        "failed"
    );
    assert_eq!(
        fs::read(runtime.inner.fixture.run.join("redis").join("launch.json")).unwrap(),
        b"{}"
    );
    assert!(
        !runtime
            .inner
            .fixture
            .run
            .join("lifecycle-0003")
            .join("partial-closed.json")
            .exists()
    );
}

#[test]
fn lost_partial_stop_response_preserves_failure_and_forbids_second_stop() {
    let mut runtime = PartialRuntime::new();
    runtime.lose_stop_response = true;
    assert!(runtime.execute("close").is_err());
    let ledger = runtime.inner.fixture.state();
    assert_eq!(ledger["attempts"][2]["status"], "failed");
    assert!(ledger["attempts"][2]["result"].is_null());
    assert!(runtime.execute("close").is_err());
    assert_eq!(
        runtime
            .inner
            .events
            .iter()
            .filter(|event| *event == "tree-stop")
            .count(),
        1
    );
}
