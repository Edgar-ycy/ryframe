use super::super::observation::{identities, observe, receipt};
use super::*;
use std::{collections::VecDeque, net::TcpListener};

struct Observer {
    fixture: Fixture,
    responses: VecDeque<&'static str>,
    clock: Duration,
    cache: &'static str,
    running: bool,
    events: Vec<(String, Value)>,
    members_fail: bool,
}
impl Observer {
    fn new() -> Self {
        let mut fixture = Fixture::new();
        for role in ["supervisor", "monitor", "process"] {
            fixture.services["tree"][role]["executable"] = json!(std::env::current_exe().unwrap());
        }
        let ports = [
            TcpListener::bind("127.0.0.1:0").unwrap(),
            TcpListener::bind("127.0.0.1:0").unwrap(),
        ];
        for (key, port) in ["api_url", "console_url"].into_iter().zip(&ports) {
            fixture.services["requests"]["rustfs"][key] = json!(format!(
                "http://127.0.0.1:{}",
                port.local_addr().unwrap().port()
            ));
        }
        Self {
            fixture,
            responses: VecDeque::new(),
            clock: Duration::ZERO,
            cache: "stopped",
            running: true,
            events: Vec::new(),
            members_fail: false,
        }
    }
    fn observe(&mut self) -> Result<Value> {
        let backend = self.fixture.backend.clone();
        let services = self.fixture.services.clone();
        observe(self, &backend, &services)
    }
    fn ready(&self) -> PathBuf {
        let tree = &self.fixture.services["tree"];
        let path = receipt(tree, "ready").unwrap();
        json_io::write_new(
            &path,
            &json!({"format_version":1,"operation_id":tree["operation_id"],
            "directory":tree["runtime_directory"],"role":tree["role"],"scope_id":tree["scope_id"],
            "supervisor":tree["supervisor"],"monitor":tree["monitor"]}),
        )
        .unwrap();
        path
    }
}
impl Runtime for Observer {
    fn call(&mut self, operation: &str, args: Value) -> Result<Value> {
        self.events.push((operation.into(), args.clone()));
        match operation {
            "cache-status" => Ok(json!({"state":self.cache})),
            "storage-running" => Ok(json!(self.running)),
            "members-wait" if self.members_fail => Err(model::error("成员关闭证明无效")),
            "members-wait" => Ok(Value::Null),
            "identity" => match self.responses.pop_front().unwrap_or("missing") {
                "denied" => Err(Box::new(model::LifecycleError {
                    category: "PermissionError",
                    message: "身份暂不可读",
                })),
                "alive" => Ok(self.fixture.services["tree"]
                    .as_object()
                    .unwrap()
                    .values()
                    .find(|value| value.is_object() && value["pid"] == args["pid"])
                    .unwrap()
                    .clone()),
                "reused" => Ok(
                    json!({"pid":args["pid"],"started":"2","executable":std::env::current_exe().unwrap()}),
                ),
                _ => Ok(Value::Null),
            },
            _ => Err(model::error("测试中出现未登记原语")),
        }
    }
    fn now(&self) -> Duration {
        self.clock
    }
    fn pause(&mut self, delay: Duration) {
        self.clock += delay;
    }
}

#[test]
fn running_members_use_successor_cache_request_and_exact_storage_identity() {
    let mut runtime = Observer::new();
    runtime.responses = VecDeque::from(["alive", "alive", "alive"]);
    runtime.fixture.services["cache_effective"] = json!({"tool":"successor"});
    let value = runtime.observe().unwrap();
    assert_eq!(
        value,
        json!({"redis":"stopped","rustfs":"running","termination":null})
    );
    assert_eq!(
        runtime.events[0].1["effective"],
        json!({"tool":"successor"})
    );
    let storage = runtime.events.last().unwrap();
    assert_eq!(storage.0, "storage-running");
    assert_eq!(
        storage.1["identity"],
        runtime.fixture.services["tree"]["process"]
    );
    runtime.responses = VecDeque::from(["alive", "alive", "alive"]);
    runtime.running = false;
    assert!(
        runtime
            .observe()
            .unwrap_err()
            .to_string()
            .contains("产品进程退出")
    );
}

#[test]
fn reused_pids_are_recorded_exits_and_partial_members_fail_closed() {
    let mut runtime = Observer::new();
    let tree = runtime.fixture.services["tree"].clone();
    runtime.responses = VecDeque::from(["missing", "reused", "missing"]);
    assert_eq!(
        identities(&mut runtime, &tree).unwrap(),
        json!({"supervisor":"missing","monitor":"missing","process":"missing"})
    );
    runtime.responses = VecDeque::from(["missing", "missing", "alive"]);
    assert!(
        runtime
            .observe()
            .unwrap_err()
            .to_string()
            .contains("部分存活")
    );
    assert!(
        !runtime
            .events
            .iter()
            .any(|(operation, _)| operation.contains("stop"))
    );
}

#[test]
fn permission_retry_is_bounded_by_one_shared_deadline() {
    let mut runtime = Observer::new();
    let tree = runtime.fixture.services["tree"].clone();
    runtime.responses = VecDeque::from(["denied", "missing", "missing", "missing"]);
    identities(&mut runtime, &tree).unwrap();
    assert_eq!(runtime.clock, Duration::from_millis(50));
    runtime.responses = std::iter::repeat_n("denied", 41)
        .chain(["missing"])
        .chain(std::iter::repeat_n("denied", 61))
        .collect();
    let before = runtime.clock;
    let error = identities(&mut runtime, &tree).unwrap_err();
    assert_eq!(model::category(error.as_ref()), "PermissionError");
    assert_eq!(runtime.clock - before, Duration::from_secs(5));
}

#[test]
fn external_termination_requires_ready_proof_redis_exit_and_second_identity_sample() {
    let mut runtime = Observer::new();
    let before = runtime.fixture.state();
    assert!(runtime.observe().is_err());
    let ready = runtime.ready();
    let observed = runtime.observe().unwrap();
    assert_eq!(observed["rustfs"], "external-termination-unreconciled");
    assert!(observed["termination"]["normal_shutdown_proof"].is_null());
    assert_eq!(
        observed["termination"]["evidence"]["rustfs_monitor_ready"],
        binding(&ready).unwrap()
    );
    assert_eq!(runtime.fixture.state(), before);
    runtime.cache = "running";
    assert!(
        runtime
            .observe()
            .unwrap_err()
            .to_string()
            .contains("Redis 仍存活")
    );
    runtime.cache = "stopped";
    runtime.responses = VecDeque::from([
        "missing", "missing", "missing", "alive", "missing", "missing",
    ]);
    assert!(
        runtime
            .observe()
            .unwrap_err()
            .to_string()
            .contains("身份变化")
    );
}

#[test]
fn external_termination_rejects_control_or_result_and_changed_ready_receipt() {
    for suffix in ["control", "result"] {
        let mut runtime = Observer::new();
        runtime.ready();
        let path = runtime.fixture.run.join(format!(
            "rustfs/rustfs-tree-{}-{suffix}.json",
            "b".repeat(32)
        ));
        std::fs::write(path, b"{}").unwrap();
        assert!(
            runtime
                .observe()
                .unwrap_err()
                .to_string()
                .contains("未完成的控制或关闭")
        );
    }
    let mut runtime = Observer::new();
    let ready = runtime.ready();
    std::fs::write(ready, b"{}").unwrap();
    assert!(
        runtime
            .observe()
            .unwrap_err()
            .to_string()
            .contains("启动归属")
    );
}

#[test]
fn stopped_receipt_requires_complete_members_and_closed_ports() {
    let mut runtime = Observer::new();
    std::fs::write(
        receipt(&runtime.fixture.services["tree"], "stopped").unwrap(),
        b"{}",
    )
    .unwrap();
    assert_eq!(runtime.observe().unwrap()["rustfs"], "stopped");
    assert!(
        runtime
            .events
            .iter()
            .any(|(operation, _)| operation == "members-wait")
    );
    runtime.members_fail = true;
    assert!(runtime.observe().is_err());
    runtime.members_fail = false;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    runtime.fixture.services["requests"]["rustfs"]["api_url"] = json!(format!(
        "http://127.0.0.1:{}",
        listener.local_addr().unwrap().port()
    ));
    assert!(runtime.observe().is_err());
}

#[test]
fn observation_rejects_escaped_directory_invalid_identity_and_unknown_cache_state() {
    let mut runtime = Observer::new();
    runtime.fixture.services["tree"]["runtime_directory"] = json!(runtime.fixture.backend);
    assert!(runtime.observe().is_err());
    assert!(runtime.events.is_empty());
    let mut runtime = Observer::new();
    runtime.fixture.services["tree"]["monitor"]["pid"] = json!(true);
    assert!(runtime.observe().is_err());
    runtime.cache = "unknown";
    assert!(
        runtime
            .observe()
            .unwrap_err()
            .to_string()
            .contains("明确结果")
    );
}
