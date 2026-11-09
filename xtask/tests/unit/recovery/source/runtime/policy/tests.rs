use super::*;
use serde_json::json;

#[test]
fn login_rows_keep_null_utf8_and_string_ids() {
    let raw = "9007199254740993\t73797374656D\tE794A8E688B7\t\tNULL\t4E6F6465\tNULL\t31\tNULL\t2026-01-01T00:00:00.000000";
    let rows = model::login_rows(&json!({"raw":raw})).unwrap();
    assert_eq!(rows[0]["id"], "9007199254740993");
    assert_eq!(rows[0]["user_name"], "用户");
    assert_eq!(rows[0]["ipaddr"], "");
    assert_eq!(rows[0]["msg"], Value::Null);
    assert_eq!(model::login_rows(&json!({"raw":""})).unwrap(), json!([]));
}

#[test]
fn login_rows_reject_duplicate_unsorted_invalid_hex_and_utf8() {
    let row = "2\t31\t31\t31\tNULL\tNULL\tNULL\t31\tNULL\ttime";
    for raw in [
        format!("{row}\n{row}"),
        row.replace("2\t", "0\t"),
        row.replace("31", "ff"),
        row.replace("31", "FF"),
        row.replace("31", "F"),
        row.replace("NULL", "null"),
        format!("{row}\n") + &row.replacen('2', "1", 1),
    ] {
        assert!(model::login_rows(&json!({"raw":raw})).is_err());
    }
}

#[test]
fn unknown_cleanup_reports_observation_without_inventing_command_result() {
    let cleanup = json!({"execution":"reconciled_unknown_exec_result","attempt_phase":"normal","deleted":null,
        "expired_snapshot_keys":[],"after":vec![json!({});33]});
    let value = model::remote_writes(&json!({"cleanup":cleanup,"recovered":true})).unwrap();
    assert_eq!(value["confirmed_total_commands"], 22);
    assert_eq!(value["authorization_cache"]["deleted_keys"], Value::Null);
    assert_eq!(
        value["authorization_cache"]["unknown_result_commands"],
        Value::Null
    );
    assert!(model::remote_writes(&json!({"cleanup":cleanup,"recovered":false})).is_err());
}

#[test]
fn known_cleanup_requires_exact_accounting() {
    let mut cleanup = json!({"execution":"confirmed_watch_exec","attempt_phase":"normal","deleted":32,
        "expired_snapshot_keys":["snapshot"],"after":vec![json!({});33]});
    let value = model::remote_writes(&json!({"cleanup":cleanup,"recovered":false})).unwrap();
    assert_eq!(value["confirmed_total_commands"], 23);
    cleanup["deleted"] = json!(33);
    assert!(model::remote_writes(&json!({"cleanup":cleanup,"recovered":false})).is_err());
}

#[test]
fn runtime_and_recovery_graphs_preserve_checkpoint_and_no_replay_boundaries() {
    let verify = flow::plan(&json!({"mode":"verify"})).unwrap();
    let recover = flow::plan(&json!({"mode":"recover"})).unwrap();
    let verify = verify.as_array().unwrap();
    let recover = recover.as_array().unwrap();
    assert_eq!(verify.iter().filter(|s| *s == "producer").count(), 1);
    assert_eq!(verify.iter().filter(|s| *s == "checkpoint").count(), 6);
    assert!(verify.windows(3).any(|w| w
        == [
            json!("checkpoint"),
            json!("publish"),
            json!("verify-published")
        ]));
    assert!(
        !recover
            .iter()
            .any(|s| s == "producer" || s == "login-before" || s == "login-after")
    );
    assert_eq!(recover.iter().filter(|s| *s == "login-recheck").count(), 2);
    assert!(recover.windows(3).any(|w| w
        == [
            json!("cache-resume"),
            json!("checkpoint"),
            json!("capture-after")
        ]));
    assert!(flow::plan(&json!({"mode":"unknown"})).is_err());
}

#[test]
fn time_chain_rejects_future_reordering_unzoned_and_empty() {
    let valid = ["2026-01-01T00:00:00Z", "2026-01-01T00:00:01+00:00"];
    model::time_order(&json!({"label":"运行","values":valid})).unwrap();
    for values in [
        json!([]),
        json!([valid[1], valid[0]]),
        json!(["2999-01-01T00:00:00Z"]),
        json!(["2026-01-01T00:00:00"]),
        json!([false]),
    ] {
        assert!(model::time_order(&json!({"label":"运行","values":values})).is_err());
    }
}

#[test]
fn producer_command_keeps_explicit_configuration_and_unicode_paths_without_secrets() {
    let args = json!({"staging":"D:/试验 目录/source-tools","backend":"D:/后端","execution":"D:/执行",
        "directory":"D:/运行/verification","operation":"a".repeat(32),"tool":"D:/工具.exe",
        "start":{"sha256":"b".repeat(64)},"lineage":{"path":"D:/血缘.json"},
        "configuration":"D:/运行/verification/source-producer-configuration.json"});
    let value = producer::command(&args).unwrap();
    let argv = value["argv"].as_array().unwrap();
    assert_eq!(argv[0], "D:/工具.exe");
    assert_eq!(argv[1], "__recovery-existing-source");
    assert!(argv.contains(&json!(
        "D:/运行/verification/source-producer-configuration.json"
    )));
    assert_eq!(argv.last().unwrap(), "--write");
    let mut invalid = args;
    invalid["operation"] = json!("BAD");
    assert!(producer::command(&invalid).is_err());
}

#[test]
fn policy_rejects_unknown_fields_and_actions_without_echoing_values() {
    for request in [
        json!({"action":"unknown","arguments":{}}),
        json!({"action":"flow","arguments":{"mode":"verify","secret":"private-marker"}}),
        json!({"action":"login-rows","arguments":{"raw":false}}),
    ] {
        let error = super::request(&request).unwrap_err().to_string();
        assert!(!error.contains("private-marker"));
    }
}
