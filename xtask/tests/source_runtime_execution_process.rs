//! 来源运行策略的实际进程入口；只读请求不能获得远端写入授权。
use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn invoke(value: &Value) -> (std::process::ExitStatus, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__source-evidence")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(value).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.stderr.is_empty());
    (
        output.status,
        serde_json::from_slice(&output.stdout).unwrap(),
    )
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
            "execution":"reconciled_unknown_lua_result","deleted":null,"expired_snapshot_keys":[],"after":vec![json!({});33]}}}});
    let (status, response) = invoke(&request);
    assert!(status.success(), "{response}");
    assert_eq!(response["result"]["value"]["confirmed_total_commands"], 22);
    assert_eq!(
        response["result"]["value"]["authorization_cache"]["deleted_keys"],
        Value::Null
    );
}

#[test]
fn runtime_adapter_plans_are_dispatched_without_relaxing_write_or_scalar_boundaries() {
    for (action, arguments) in [
        ("cache-result", json!({"value":[27,["expired"]]})),
        (
            "login-query",
            json!({"database":"control","selection":"new","upper_id":9007199254740993_u64}),
        ),
        ("receipt-fields", json!({"recovered":true})),
        ("failed-entries", json!({})),
    ] {
        let (status, response) = invoke(&json!({"operation":"producer-ready","write":false,
            "arguments":{"action":action,"arguments":arguments}}));
        assert!(status.success(), "{action}: {response}");
        match action {
            "cache-result" => assert_eq!(
                response["result"]["value"],
                json!({"deleted":27,"expired":["expired"]})
            ),
            "login-query" => assert!(
                response["result"]["value"]
                    .as_str()
                    .unwrap()
                    .contains("WHERE `id` > 9007199254740993")
            ),
            "receipt-fields" => assert!(
                response["result"]["value"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("recovery"))
            ),
            "failed-entries" => assert!(
                !response["result"]["value"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("after"))
            ),
            _ => unreachable!(),
        }
    }
}

#[test]
fn runtime_new_plans_reject_secrets_unknown_fields_and_unconfirmed_results() {
    for (action, arguments) in [
        ("cache-result", json!({"value":[true,[]]})),
        ("cache-result", json!({"value":null})),
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
