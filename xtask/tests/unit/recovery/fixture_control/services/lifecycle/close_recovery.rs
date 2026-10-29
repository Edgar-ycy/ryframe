use super::*;
use crate::recovery::clone::native::digest;

struct Recovery {
    inner: FakeRuntime,
    port_error: Option<std::io::ErrorKind>,
    late_success: bool,
    change_on_port: bool,
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
            true,
        )
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
        match operation {
            "identity" => {
                for role in ["supervisor", "monitor", "process"] {
                    let expected = &self.inner.fixture.services["tree"][role];
                    if expected["pid"] == args["pid"] {
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
                Err(model::error("停止后结果未发布"))
            }
            "cache-status" => Ok(
                json!({"state":"stopped","launcher_alive":false,"linux":{"identity":args["runtime"]["linux_identity"],"alive":false,"boot_id":args["runtime"]["boot_id"],"terminated":false}}),
            ),
            "cache-closed-port" => {
                self.inner.events.push(operation.into());
                if self.change_on_port {
                    fs::write(&self.inner.fixture.review, b"changed")?;
                }
                if self.late_success {
                    self.inner.clock += Duration::from_secs(180);
                }
                match self.port_error {
                    Some(kind) => Err(std::io::Error::from(kind).into()),
                    None => Ok(json!(true)),
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
