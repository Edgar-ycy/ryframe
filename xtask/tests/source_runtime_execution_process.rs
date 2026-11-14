//! 来源运行策略的实际进程入口；只读请求不能获得远端写入授权。
use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Child, Command, ExitStatus, Output, Stdio},
    thread,
};

#[path = "source_runtime_execution_process/cache_attempt.rs"]
mod cache_attempt;

fn start() -> Child {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__source-evidence")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn finish(mut child: Child, value: &Value) -> Output {
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(value).unwrap())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn parse(output: &Output) -> (ExitStatus, Value) {
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (
        output.status,
        serde_json::from_slice(&output.stdout).unwrap(),
    )
}

fn invoke(value: &Value) -> (ExitStatus, Value) {
    parse(&finish(start(), value))
}

fn race_cache_cleanup(fixture: &cache_attempt::Fixture) {
    let request = &fixture.cleanup;
    let first = start();
    let second = start();
    let outputs = thread::scope(|scope| {
        let first = scope.spawn(move || finish(first, request));
        let second = finish(second, request);
        [first.join().unwrap(), second]
    });
    for (index, output) in outputs.iter().enumerate() {
        fixture.save_exchange(&format!("cleanup-{index}"), request, output);
    }
    let responses = outputs.each_ref().map(parse);
    for (status, response) in &responses {
        assert_eq!(response["ok"], status.success(), "{response}");
    }
    assert_eq!(
        responses
            .iter()
            .filter(|(status, _)| status.success())
            .count(),
        1,
        "{responses:?}"
    );
    let response = &responses
        .iter()
        .find(|(status, _)| status.success())
        .unwrap()
        .1;
    let outcome = &response["result"]["value"];
    assert_eq!(outcome["status"], "pre_exec_io_failed", "{outcome}");
    assert_eq!(outcome["remote_write_may_have_occurred"], false);
    assert_eq!(outcome["automatic_retry_permitted"], false);
}

#[test]
fn source_cache_attempt_is_unique_across_processes_and_pre_exec_failure_never_replays() {
    let fixture = cache_attempt::Fixture::new();
    let attempt = fixture.directory.join("cache-cleanup-attempt.json");
    let execution = fixture.directory.join("cache-cleanup-execution.json");
    let cleanup = fixture.directory.join("cache-cleanup.json");
    assert!(!attempt.exists() && !execution.exists() && !cleanup.exists());

    race_cache_cleanup(&fixture);

    let attempt_before = cache_attempt::binding(&attempt);
    let execution_before = cache_attempt::binding(&execution);
    let attempt_value = cache_attempt::read(&attempt);
    let execution_value = cache_attempt::read(&execution);
    let intent = cache_attempt::read(&fixture.intent);
    assert_eq!(
        attempt_value["recovery_intent"],
        cache_attempt::binding(&fixture.intent)
    );
    assert_eq!(attempt_value["scope_id"], cache_attempt::SCOPE);
    for key in [
        "source_generation",
        "dataset_lineage",
        "producer",
        "login_audit",
    ] {
        assert_eq!(attempt_value["bindings"][key], intent[key], "{key}");
    }
    assert_eq!(
        attempt_value["observation"],
        fixture.cleanup["arguments"]["observation"]
    );
    assert_eq!(execution_value["attempt"], attempt_before);
    assert_eq!(execution_value["status"], "pre_exec_failed");
    assert_eq!(execution_value["cause"], "pre_exec_io_failed");
    assert_eq!(execution_value["deleted"], Value::Null);
    assert_eq!(execution_value["expired"], json!([]));

    let request =
        json!({"operation":"runtime-cache-state","write":false,"arguments":fixture.frame});
    let output = finish(start(), &request);
    fixture.save_exchange("state", &request, &output);
    let (status, state) = parse(&output);
    assert!(status.success(), "{state}");
    let state = &state["result"]["value"];
    assert_eq!(state["attempt_phase"], "recovery");
    assert_eq!(state["status"], "pre_exec_failed");
    assert_eq!(state["attempt"], attempt_before);
    assert_eq!(state["execution"], execution_before);
    assert_eq!(state["result"], execution_value);
    assert_eq!(state["observation"], attempt_value["observation"]);

    let mut arguments = fixture.frame.clone();
    arguments["frozen"] = attempt_value["observation"].clone();
    arguments["types"] = json!(vec!["none"; 33]);
    arguments["observed"] = Value::Null;
    let request = json!({"operation":"runtime-cache-resume","write":false,"arguments":arguments});
    let output = finish(start(), &request);
    fixture.save_exchange("resume", &request, &output);
    let (status, response) = parse(&output);
    assert!(!status.success(), "{response}");
    assert_eq!(response["ok"], false);
    let error = &response["error"];
    assert!(
        error["message"].as_str().unwrap().contains("明确未执行"),
        "{response}"
    );
    assert_eq!(error["remote_write_may_have_occurred"], false);
    assert_eq!(error["automatic_retry_permitted"], false);
    assert_eq!(cache_attempt::binding(&attempt), attempt_before);
    assert_eq!(cache_attempt::binding(&execution), execution_before);
    assert!(!cleanup.exists());
    assert!(
        !fixture
            .directory
            .join("source-verification-recovery.json")
            .exists()
    );
    assert!(!fixture.directory.join("after").exists());
}

#[test]
fn source_runtime_dispatch_preserves_no_producer_replay_and_explicit_write_boundary() {
    let mut request = json!({"operation":"producer-ready","write":false,
        "arguments":{"action":"flow","arguments":{"mode":"recover"}}});
    let (status, response) = invoke(&request);
    assert!(status.success(), "{response}");
    let stages = response["result"]["value"].as_array().unwrap();
    assert!(!stages.iter().any(|stage| stage == "producer"));
    assert!(stages.contains(&json!("cache-resume")));
    request["write"] = json!(true);
    let (status, response) = invoke(&request);
    assert!(!status.success());
    assert_eq!(response["error"]["remote_write_may_have_occurred"], false);
    assert_eq!(response["error"]["automatic_retry_permitted"], false);
    for operation in ["runtime-cache-cleanup", "source-runtime-stopped"] {
        for write in [false, true] {
            let (status, response) = invoke(&json!({"operation":operation,
                "write":write,"arguments":{"private-source-secret":true}}));
            assert!(!status.success());
            assert_eq!(response["error"]["remote_write_may_have_occurred"], false);
            assert!(!response.to_string().contains("private-source-secret"));
        }
    }
}

#[test]
fn runtime_models_reject_private_payload_without_echoing_it() {
    for arguments in [
        json!({"action":"snapshot","arguments":{"payload":"private-source-secret","subject":{},"tenant_epoch":"1"}}),
        json!({"action":"time-order","arguments":{"label":"private-source-secret","values":["private-source-secret"]}}),
        json!({"action":"producer-command","arguments":{"operation":"private-source-secret"}}),
    ] {
        let (status, response) =
            invoke(&json!({"operation":"producer-ready","write":false,"arguments":arguments}));
        assert!(!status.success());
        assert!(!response.to_string().contains("private-source-secret"));
    }
}

#[test]
fn runtime_unknown_cache_result_does_not_synthesize_a_deletion_count() {
    let request = json!({"operation":"producer-ready","write":false,"arguments":{
        "action":"remote-writes","arguments":{"recovered":true,"cleanup":{
            "execution":"reconciled_unknown_exec_result","attempt_phase":"normal","deleted":null,"expired_snapshot_keys":[],"after":vec![json!({});33]}}}});
    let (status, response) = invoke(&request);
    assert!(status.success(), "{response}");
    assert_eq!(response["result"]["value"]["confirmed_total_commands"], 22);
    assert_eq!(
        response["result"]["value"]["authorization_cache"]["deleted_keys"],
        Value::Null
    );
}

#[test]
fn runtime_new_plans_reject_secrets_unknown_fields_and_unconfirmed_results() {
    for (action, arguments) in [
        (
            "login-query",
            json!({"database":"private-source-secret","selection":"new","upper_id":true}),
        ),
        (
            "control-database",
            json!({"databases":[{"key":"shared-control","kind":"private-source-secret"}]}),
        ),
        ("receipt-fields", json!({"recovered":1})),
        ("failed-entries", json!({"extra":"private-source-secret"})),
    ] {
        let (status, response) = invoke(&json!({"operation":"producer-ready","write":false,
            "arguments":{"action":action,"arguments":arguments}}));
        assert!(!status.success(), "{action}");
        assert_eq!(response["error"]["remote_write_may_have_occurred"], false);
        assert_eq!(response["error"]["automatic_retry_permitted"], false);
        assert!(!response.to_string().contains("private-source-secret"));
    }
}
