use super::*;
use crate::recovery::clone::native::digest;

struct Recovery {
    inner: FakeRuntime,
    port_error: Option<std::io::ErrorKind>,
    late_success: bool,
    change_on_port: bool,
    close_tree_on_stop: bool,
    member_state: Option<&'static str>,
    proof_change: Option<&'static str>,
    proof_changed: bool,
    mirrored: bool,
    last_linux_bind: Option<Duration>,
    first_windows_bind: Option<Duration>,
    last_windows_bind: Option<Duration>,
}
impl Recovery {
    fn new() -> Self {
        let mut inner = FakeRuntime::new();
        inner.fixture.services["requests"]["redis"]["format_version"] = json!(2);
        let request = inner.fixture.services["requests"]["redis"].clone();
        json_io::write_replace(&inner.fixture.run.join("redis/request.json"), &request).unwrap();
        let runtime = &mut inner.fixture.services["runtime"];
        runtime["request_sha256"] = json!(digest::plan_hash(&request).unwrap());
        runtime["boot_id"] = runtime["linux_identity"]["boot_id"].clone();
        runtime["launcher"] = json!({"pid":2147481003u32,"started":"1","executable":"fixture.exe"});
        json_io::write_replace(&inner.fixture.run.join("redis/runtime.json"), runtime).unwrap();
        for name in [
            "launch-request.json",
            "intent.json",
            "launcher.json",
            "linux-intent.json",
            "linux-process.json",
            "acknowledged.json",
            "accepted.json",
        ] {
            json_io::write_new(
                &inner.fixture.run.join("redis").join(name),
                &json!({"fixture":true}),
            )
            .unwrap();
        }
        Self {
            inner,
            port_error: None,
            late_success: false,
            change_on_port: false,
            close_tree_on_stop: false,
            member_state: None,
            proof_change: None,
            proof_changed: false,
            mirrored: false,
            last_linux_bind: None,
            first_windows_bind: None,
            last_windows_bind: None,
        }
    }
    fn execute(&mut self, operation: &str) -> Result<Value> {
        let fixture = &self.inner.fixture;
        let (backend, review, bootstrap, owner) = (
            fixture.backend.clone(),
            fixture.review.clone(),
            fixture.bootstrap.clone(),
            fixture.run.join("state.json"),
        );
        execute_with(
            self,
            &backend,
            operation,
            &review,
            &bootstrap,
            (operation == "reconcile").then_some(owner.as_path()),
            operation != "status",
        )
    }
    fn closed_tree(&mut self, records: bool) {
        let tree = self.inner.fixture.services["tree"].clone();
        self.inner.stopped_members(&tree);
        self.inner.observation["rustfs"] = json!("stopped");
        if records {
            let root = Path::new(text(&tree, "runtime_directory").unwrap());
            let operation = text(&tree, "operation_id").unwrap();
            for (suffix, value) in [
                (
                    "control",
                    json!({"format_version":1,"kind":"full-stack-process-tree-control",
                    "operation_id":operation,"mode":"normal","requested_at_ns":1}),
                ),
                (
                    "result",
                    json!({"format_version":1,"kind":"full-stack-process-tree-result",
                    "operation_id":operation,"process":tree["process"],"exit_code":0,"termination":"normal"}),
                ),
            ] {
                json_io::write_new(
                    &root.join(format!("rustfs-tree-{operation}-{suffix}.json")),
                    &value,
                )
                .unwrap();
            }
        }
    }
    fn change_tree_proof(&mut self) -> Result<()> {
        let Some(change) = self.proof_change.take() else {
            return Ok(());
        };
        let tree = &self.inner.fixture.services["tree"];
        let root = Path::new(text(tree, "runtime_directory")?);
        let operation = text(tree, "operation_id")?;
        let path = root.join(format!("rustfs-members-{operation}-stopped.json"));
        match change {
            "bytes" => {
                let mut bytes = fs::read(&path)?;
                bytes.push(b' ');
                fs::write(&path, bytes)?;
            }
            "identity" => {
                let modified = fs::metadata(&path)?.modified()?;
                let bytes = fs::read(&path)?;
                fs::rename(&path, path.with_extension("replaced"))?;
                fs::write(&path, bytes)?;
                fs::File::options()
                    .write(true)
                    .open(&path)?
                    .set_times(fs::FileTimes::new().set_modified(modified))?;
            }
            "mtime" => {
                let modified = fs::metadata(&path)?.modified()? + Duration::from_secs(1);
                fs::File::options()
                    .write(true)
                    .open(&path)?
                    .set_times(fs::FileTimes::new().set_modified(modified))?;
            }
            "result" => {
                let path = root.join(format!("rustfs-tree-{operation}-result.json"));
                let mut value = read(&path)?;
                value["exit_code"] = json!(1);
                json_io::write_replace(&path, &value)?;
            }
            _ => {
                json_io::write_new(
                    &root.join(format!("rustfs-tree-{operation}-control.json")),
                    &json!({"format_version":1,"kind":"full-stack-process-tree-control",
                        "operation_id":operation,"mode":"normal","requested_at_ns":1}),
                )?;
            }
        }
        self.proof_changed = true;
        Ok(())
    }
    fn failed_close(&mut self) {
        assert!(self.execute("close").is_err());
        assert_eq!(
            self.inner.fixture.state()["attempts"][3]["status"],
            "failed"
        );
        assert_eq!(
            fs::read_dir(self.inner.fixture.run.join("lifecycle-0004"))
                .unwrap()
                .count(),
            0
        );
    }
}
impl Runtime for Recovery {
    fn now(&self) -> Duration {
        self.inner.clock
    }
    fn pause(&mut self, delay: Duration) {
        self.inner.pause(delay)
    }
    fn call(&mut self, operation: &str, args: Value) -> Result<Value> {
        if self.mirrored
            && matches!(operation, "cache-status" | "observe")
            && self.inner.observation["redis"] == "stopped"
        {
            self.last_linux_bind = Some(self.inner.clock);
        }
        match operation {
            "identity" => {
                if self.proof_change == Some("late-proof") {
                    self.proof_change = None;
                    self.closed_tree(false);
                    self.proof_changed = true;
                }
                for role in ["supervisor", "monitor", "process"] {
                    let expected = &self.inner.fixture.services["tree"][role];
                    if expected["pid"] == args["pid"] {
                        if role == "monitor" && self.member_state == Some("reused") {
                            let mut reused = expected.clone();
                            reused["started"] = json!("reused");
                            return Ok(reused);
                        }
                        if self.member_state == Some("alive")
                            || (role == "process" && self.member_state == Some("partial"))
                        {
                            return Ok(expected.clone());
                        }
                        return Ok(if self.inner.observation["rustfs"] == "running" {
                            expected.clone()
                        } else {
                            Value::Null
                        });
                    }
                }
                Ok(Value::Null)
            }
            "storage-running" => Ok(json!(self.inner.observation["rustfs"] == "running")),
            "cache-stop" => {
                self.inner.events.push(operation.into());
                json_io::write_new(
                    &self.inner.fixture.run.join("redis/linux-exited.json"),
                    &json!({"identity":args["runtime"]["linux_identity"],"returncode":0}),
                )?;
                self.inner.observation["redis"] = json!("stopped");
                if self.close_tree_on_stop {
                    self.closed_tree(false);
                }
                Err(model::error("停止后结果未发布"))
            }
            "cache-status" => Ok(
                json!({"state":"stopped","launcher_alive":false,"linux":{"identity":args["runtime"]["linux_identity"],"alive":false,"boot_id":args["runtime"]["boot_id"],"terminated":false}}),
            ),
            "cache-closed-port" => {
                self.inner.events.push(operation.into());
                self.change_tree_proof()?;
                if self.change_on_port {
                    fs::write(&self.inner.fixture.review, b"changed")?;
                }
                if self.late_success {
                    self.inner.clock += Duration::from_secs(180);
                }
                if self.mirrored
                    && self.last_linux_bind.is_some_and(|last| {
                        self.inner.clock.saturating_sub(last) < Duration::from_secs(60)
                    })
                {
                    return Err(std::io::Error::from(std::io::ErrorKind::AddrInUse).into());
                }
                match self.port_error {
                    Some(kind) => Err(std::io::Error::from(kind).into()),
                    None => {
                        let _ = self.first_windows_bind.get_or_insert(self.inner.clock);
                        self.last_windows_bind = Some(self.inner.clock);
                        Ok(json!(true))
                    }
                }
            }
            _ => self.inner.call(operation, args),
        }
    }
}

#[test]
fn failed_close_recovery_never_replays_stop_preserves_failure_and_closes_original_tree() {
    let mut runtime = Recovery::new();
    runtime.failed_close();
    let before = runtime.inner.fixture.state()["attempts"].clone();
    let value = runtime.execute("reconcile").unwrap();
    assert_eq!(value["status"], "failed_close_reconciled");
    assert_eq!(
        &runtime.inner.fixture.state()["attempts"]
            .as_array()
            .unwrap()[..4],
        before.as_array().unwrap()
    );
    assert_eq!(
        runtime
            .inner
            .events
            .iter()
            .filter(|event| *event == "cache-stop")
            .count(),
        1
    );
    assert_eq!(
        fs::read_dir(runtime.inner.fixture.run.join("lifecycle-0004"))
            .unwrap()
            .count(),
        0
    );
    assert!(
        runtime
            .inner
            .fixture
            .run
            .join("lifecycle-0005/close-recovery.json")
            .exists()
    );
    assert!(
        !runtime
            .inner
            .fixture
            .run
            .join("lifecycle-0005/stopped.json")
            .exists()
    );
    assert_eq!(runtime.inner.fixture.history()["closed"], true);
    let proof_path = runtime
        .inner
        .fixture
        .run
        .join("lifecycle-0005/close-recovery.json");
    let mut proof = read(&proof_path).unwrap();
    proof["candidate"]["failed_attempt"] = json!(3);
    json_io::write_replace(&proof_path, &proof).unwrap();
    assert!(
        history::validate(
            &runtime.inner.fixture.run,
            &runtime.inner.fixture.state(),
            true,
            true
        )
        .is_err()
    );
}

#[test]
fn recovery_rejects_missing_exit_unknown_writes_identity_or_source_drift_without_tree_stop() {
    for scenario in [
        "missing-exit",
        "wrong-exit",
        "unknown-write",
        "tree-identity",
        "source-drift",
        "exit-before",
        "controller",
    ] {
        let mut runtime = Recovery::new();
        runtime.failed_close();
        match scenario {
            "missing-exit" => {
                fs::remove_file(runtime.inner.fixture.run.join("redis/linux-exited.json")).unwrap()
            }
            "wrong-exit" => json_io::write_replace(
                &runtime.inner.fixture.run.join("redis/linux-exited.json"),
                &json!({"identity":{},"returncode":0}),
            )
            .unwrap(),
            "unknown-write" => fs::write(
                runtime
                    .inner
                    .fixture
                    .run
                    .join("lifecycle-0004/unknown.json"),
                b"{}",
            )
            .unwrap(),
            "tree-identity" => runtime.inner.observation["rustfs"] = json!("unknown"),
            "exit-before" => {
                let file = std::fs::File::options()
                    .write(true)
                    .open(runtime.inner.fixture.run.join("redis/linux-exited.json"))
                    .unwrap();
                file.set_times(
                    std::fs::FileTimes::new()
                        .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1)),
                )
                .unwrap();
            }
            "controller" => runtime.inner.controller = json!({"process_matches":true}),
            _ => runtime.change_on_port = true,
        }
        assert!(runtime.execute("reconcile").is_err(), "{scenario}");
        assert!(
            !runtime
                .inner
                .events
                .iter()
                .any(|event| event == "tree-stop"),
            "{scenario}"
        );
        assert_eq!(
            runtime
                .inner
                .events
                .iter()
                .filter(|event| *event == "cache-stop")
                .count(),
            1
        );
    }
}

#[test]
fn recovery_port_budget_is_fixed_and_other_errors_or_deadline_success_never_stop_tree() {
    for (error, late) in [
        (Some(std::io::ErrorKind::AddrInUse), false),
        (Some(std::io::ErrorKind::PermissionDenied), false),
        (None, true),
    ] {
        let mut runtime = Recovery::new();
        runtime.failed_close();
        runtime.port_error = error;
        runtime.late_success = late;
        assert!(runtime.execute("reconcile").is_err());
        assert!(
            !runtime
                .inner
                .events
                .iter()
                .any(|event| event == "tree-stop")
        );
        if error == Some(std::io::ErrorKind::AddrInUse) {
            assert_eq!(runtime.inner.clock, Duration::from_secs(180));
        }
        if error == Some(std::io::ErrorKind::PermissionDenied) {
            assert_eq!(runtime.inner.pauses, 0);
        }
    }
}

#[test]
fn recovery_port_budget_allows_mirrored_release_without_rearming_linux_on_every_retry() {
    let mut runtime = Recovery::new();
    runtime.failed_close();
    runtime.closed_tree(false);
    runtime.mirrored = true;
    let before = runtime.inner.fixture.state()["attempts"].clone();
    let exited = runtime.inner.fixture.run.join("redis/linux-exited.json");
    let exit_before = fs::read(&exited).unwrap();
    let members = runtime
        .inner
        .stopped_members(&runtime.inner.fixture.services["tree"]);
    let value = runtime.execute("reconcile").unwrap();
    assert_eq!(value["status"], "failed_close_reconciled");
    let first_windows = runtime.first_windows_bind.expect("首段Windows独占证明");
    let final_linux = runtime.last_linux_bind.expect("实际双侧Linux证明");
    let final_windows = runtime.last_windows_bind.expect("末段Windows独占证明");
    assert!(first_windows >= Duration::from_secs(60));
    assert!(
        final_linux >= first_windows,
        "首段Windows证明后仍须完整Linux复核"
    );
    assert!(final_windows.saturating_sub(final_linux) >= Duration::from_secs(60));
    assert!(runtime.inner.clock < Duration::from_secs(180));
    assert_eq!(
        &runtime.inner.fixture.state()["attempts"]
            .as_array()
            .unwrap()[..4],
        before.as_array().unwrap()
    );
    assert_eq!(fs::read(&exited).unwrap(), exit_before);
    assert_eq!(
        runtime
            .inner
            .stopped_members(&runtime.inner.fixture.services["tree"]),
        members
    );
    assert_eq!(
        fs::read_dir(runtime.inner.fixture.run.join("lifecycle-0004"))
            .unwrap()
            .count(),
        0
    );
    assert!(
        !runtime
            .inner
            .events
            .iter()
            .any(|event| event == "tree-stop")
    );
    assert_eq!(
        runtime
            .inner
            .events
            .iter()
            .filter(|event| *event == "cache-stop")
            .count(),
        1
    );
    assert_eq!(runtime.inner.fixture.history()["closed"], true);
}

#[test]
fn recovery_requires_exact_owner_and_unknown_tree_stop_is_never_replayed() {
    let mut runtime = Recovery::new();
    runtime.failed_close();
    let fixture = &runtime.inner.fixture;
    let (backend, review, bootstrap) = (
        fixture.backend.clone(),
        fixture.review.clone(),
        fixture.bootstrap.clone(),
    );
    let alias = fixture.run.join("owner-alias.json");
    fs::copy(fixture.run.join("state.json"), &alias).unwrap();
    assert!(
        execute_with(
            &mut runtime,
            &backend,
            "reconcile",
            &review,
            &bootstrap,
            Some(&alias),
            true
        )
        .is_err()
    );
    assert_eq!(
        state::attempts(&runtime.inner.fixture.state())
            .unwrap()
            .len(),
        4
    );
    runtime.inner.fail = Some("tree-stop".into());
    assert!(runtime.execute("reconcile").is_err());
    assert_eq!(
        runtime.inner.fixture.state()["attempts"][4]["status"],
        "failed"
    );
    let before = runtime
        .inner
        .events
        .iter()
        .filter(|event| *event == "tree-stop")
        .count();
    runtime.inner.fail = None;
    assert!(runtime.execute("reconcile").is_err());
    assert_eq!(
        runtime
            .inner
            .events
            .iter()
            .filter(|event| *event == "tree-stop")
            .count(),
        before
    );
    assert_eq!(
        runtime
            .inner
            .events
            .iter()
            .filter(|event| *event == "cache-stop")
            .count(),
        1
    );
    assert_eq!(
        fs::read_dir(runtime.inner.fixture.run.join("lifecycle-0004"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn stopped_tree_before_during_or_after_failed_close_reconciles_without_control_replay() {
    for timing in ["before", "during", "after"] {
        let mut runtime = Recovery::new();
        if timing == "before" {
            runtime.closed_tree(false);
        }
        runtime.close_tree_on_stop = timing == "during";
        runtime.failed_close();
        if timing == "after" {
            runtime.closed_tree(true);
        }
        let tree = &runtime.inner.fixture.services["tree"];
        let root = PathBuf::from(text(tree, "runtime_directory").unwrap());
        let snapshots = fs::read_dir(&root)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let bytes = fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect::<Vec<_>>();
        let prefix = runtime.inner.fixture.state()["attempts"].clone();
        assert_eq!(
            runtime.execute("status").unwrap()["next_operation"],
            "reconcile"
        );
        assert_eq!(runtime.inner.fixture.state()["attempts"], prefix);
        let value = runtime.execute("reconcile").unwrap();
        assert_eq!(value["status"], "failed_close_reconciled", "{timing}");
        assert_eq!(
            &runtime.inner.fixture.state()["attempts"]
                .as_array()
                .unwrap()[..4],
            prefix.as_array().unwrap()
        );
        assert!(runtime.inner.fixture.history()["closed"].as_bool().unwrap());
        assert!(
            !runtime
                .inner
                .events
                .iter()
                .any(|event| event == "tree-stop")
        );
        assert_eq!(
            runtime
                .inner
                .events
                .iter()
                .filter(|event| *event == "cache-stop")
                .count(),
            1
        );
        assert_eq!(
            fs::read_dir(runtime.inner.fixture.run.join("lifecycle-0004"))
                .unwrap()
                .count(),
            0
        );
        for (path, bytes) in snapshots {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
        let recovery = read(
            &runtime
                .inner
                .fixture
                .run
                .join("lifecycle-0005/close-recovery.json"),
        )
        .unwrap();
        assert_eq!(
            recovery["rustfs_members"],
            runtime
                .inner
                .stopped_members(&runtime.inner.fixture.services["tree"])
        );
        assert!(runtime.inner.fixture.history()["restart_predecessor"].is_object());
    }
}

#[test]
fn stopped_tree_requires_valid_original_completion_and_exact_exit_of_every_member() {
    for scenario in [
        "missing",
        "foreign",
        "failed",
        "members",
        "partial",
        "reused",
        "alive",
        "control",
        "result",
        "unknown-record",
    ] {
        let mut runtime = Recovery::new();
        runtime.failed_close();
        runtime.closed_tree(true);
        let tree = &runtime.inner.fixture.services["tree"];
        let root = PathBuf::from(text(tree, "runtime_directory").unwrap());
        let operation = text(tree, "operation_id").unwrap();
        let path = root.join(format!("rustfs-members-{operation}-stopped.json"));
        match scenario {
            "missing" => fs::remove_file(&path).unwrap(),
            "foreign" | "failed" | "members" => {
                let mut proof = read(&path).unwrap();
                match scenario {
                    "foreign" => proof["monitor"]["started"] = json!("foreign"),
                    "failed" => proof["error_type"] = json!("Unknown"),
                    _ => proof["members"] = json!([]),
                }
                json_io::write_replace(&path, &proof).unwrap();
            }
            "control" | "result" => {
                let path = root.join(format!("rustfs-tree-{operation}-{scenario}.json"));
                let mut proof = read(&path).unwrap();
                proof["operation_id"] = json!("foreign");
                json_io::write_replace(&path, &proof).unwrap();
            }
            "unknown-record" => {
                fs::write(root.join("rustfs-tree-unknown-control.json"), b"{}").unwrap()
            }
            _ => runtime.member_state = Some(scenario),
        }
        let before = runtime.inner.fixture.state();
        assert_eq!(
            runtime.execute("status").unwrap()["next_operation"],
            "reconcile-evidence",
            "{scenario}"
        );
        assert_eq!(runtime.inner.fixture.state(), before);
        assert!(runtime.execute("reconcile").is_err(), "{scenario}");
        assert_eq!(runtime.inner.fixture.state(), before);
        assert!(
            !runtime
                .inner
                .events
                .iter()
                .any(|event| event == "tree-stop")
        );
        assert_eq!(
            runtime
                .inner
                .events
                .iter()
                .filter(|event| *event == "cache-stop")
                .count(),
            1
        );
    }
}

#[test]
fn stopped_tree_rejects_new_proof_or_changed_bytes_file_identity_and_record_set() {
    for change in [
        "late-proof",
        "bytes",
        "identity",
        "mtime",
        "control",
        "result",
    ] {
        let mut runtime = Recovery::new();
        runtime.failed_close();
        if change != "late-proof" {
            runtime.closed_tree(change != "control");
        } else {
            runtime.inner.observation["rustfs"] = json!("stopped");
        }
        runtime.proof_change = Some(change);
        assert!(runtime.execute("reconcile").is_err(), "{change}");
        assert!(runtime.proof_changed, "注入必须真实完成：{change}");
        assert!(
            !runtime
                .inner
                .events
                .iter()
                .any(|event| event == "tree-stop"),
            "{change}"
        );
        assert_eq!(
            runtime
                .inner
                .events
                .iter()
                .filter(|event| *event == "cache-stop")
                .count(),
            1
        );
        assert_eq!(
            fs::read_dir(runtime.inner.fixture.run.join("lifecycle-0004"))
                .unwrap()
                .count(),
            0
        );
        assert_ne!(
            runtime.inner.fixture.state()["attempts"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["status"],
            "passed"
        );
    }
}
