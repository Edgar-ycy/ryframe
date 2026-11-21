use super::*;
use crate::recovery::clone::native::seed::arm_input::history as arm_history;
use std::fs;

fn row(n: u64, stage: &str, mode: &str, status: &str) -> Value {
    json!({"number":n,"stage":stage,"mode":mode,"status":status,"result":{"attempt":n}})
}

fn prefix() -> Vec<Value> {
    vec![
        row(52, "seed-runtime", "source-register", "passed"),
        row(53, "storage-target", "restart", "passed"),
    ]
}

fn args(rows: Vec<Value>) -> Value {
    json!({"attempts":rows, "descriptor":{"attempt":52}, "current":null,
        "archive":null,"segment":null,"pending":[],"backend_available":true,
        "cache_recovery":false,"generation_output_exists":false})
}

fn rebound() -> Vec<Value> {
    let mut rows = prefix();
    rows.push(row(54, "seed-runtime", "source-rebind", "passed"));
    rows
}

fn generation() -> Vec<Value> {
    let mut rows = rebound();
    rows.push(row(55, "seed-runtime", "source-generation-start", "passed"));
    rows.push(row(56, "seed-runtime", "source-generation-stop", "passed"));
    rows
}

fn accepted(args: &Value) -> Value {
    let value = history::plan(args).unwrap();
    assert!(value.get("rejected").is_none(), "{value}");
    value
}

fn rejected(args: &Value) {
    assert!(match history::plan(args) {
        Err(_) => true,
        Ok(value) => value.get("rejected").is_some(),
    });
}

#[test]
fn unique_publication_and_monotonic_history_are_required() {
    let mut rows = prefix();
    let unchanged = rows.clone();
    assert_eq!(accepted(&args(rows.clone()))["later"], json!([rows[1]]));
    rows.push(row(54, "seed-runtime", "source-register", "passed"));
    rows[2]["result"] = json!({"attempt":52});
    rejected(&args(rows));
    let mut input = args(unchanged.clone());
    input["descriptor"] = json!({"other":true});
    rejected(&input);
    let mut rows = unchanged;
    rows[1]["number"] = json!(52);
    rejected(&args(rows));
}

#[test]
fn unknown_failed_duplicate_and_post_rebind_mutations_fail_closed() {
    for (stage, mode, status) in [
        ("copy", "run", "passed"),
        ("storage-target", "restart", "passed"),
        ("storage-target", "stop", "passed"),
        ("seed-runtime", "source-rebind", "passed"),
        ("seed-runtime", "arm-input", "running"),
        ("seed-runtime", "start", "running"),
    ] {
        let mut rows = rebound();
        rows.push(row(55, stage, mode, status));
        rejected(&args(rows));
    }
    let mut rows = prefix();
    rows.push(row(54, "seed-runtime", "source-rebind", "failed"));
    rejected(&args(rows));
}

#[test]
fn failed_arm_requests_exact_prefix_proof_and_cannot_proceed_without_it() {
    let mut rows = rebound();
    let mut failed = row(55, "seed-runtime", "arm-input", "failed");
    failed["result"] = Value::Null;
    rows.push(failed.clone());
    let input = args(rows.clone());
    assert_eq!(
        accepted(&input),
        json!({"later":&rows[1..],"actions":[{"kind":"arm-prefix","record":failed}]})
    );

    let backend = crate::workspace::root_dir();
    let local = backend.join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let run = tempfile::tempdir_in(local).unwrap();
    let error = arm_history::prove(
        &backend,
        run.path(),
        &input,
        &failed,
        &input["descriptor"],
        None,
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "seed 历史包含未受控的 ARM 失败前缀");
    assert!(fs::read_dir(run.path()).unwrap().next().is_none());
}

#[test]
fn current_requires_exact_last_running_operation_and_requests_owner_proof() {
    let mut rows = prefix();
    rows.push(row(54, "seed-runtime", "source-rebind", "running"));
    let mut input = args(rows);
    input["current"] = json!(54);
    assert_eq!(accepted(&input)["actions"], json!([{"kind":"owner"}]));
    for current in [json!(53), json!(null), json!(true)] {
        input["current"] = current;
        rejected(&input);
    }
}

#[test]
fn export_requires_unique_rebind_start_and_stop() {
    let export = row(57, "seed-runtime", "source-export", "passed");
    let mut rows = generation();
    rows.push(export.clone());
    accepted(&args(rows.clone()));
    let mut active = args(rows.clone());
    active["attempts"][5]["status"] = json!("running");
    active["current"] = json!(57);
    assert_eq!(accepted(&active)["actions"], json!([{"kind":"owner"}]));
    rows.push(row(58, "seed-runtime", "source-export", "passed"));
    rejected(&args(rows));
    let mut rows = prefix();
    rows.push(export);
    rejected(&args(rows));
}

#[test]
fn completed_generation_cannot_be_recovered_or_reused_for_export() {
    let mut rows = generation();
    rows.push(row(
        57,
        "seed-runtime",
        "source-generation-recover",
        "passed",
    ));
    rows.push(row(58, "seed-runtime", "source-export", "passed"));
    assert!(
        history::plan(&args(rows)).unwrap()["rejected"]
            .as_str()
            .unwrap()
            .contains("不能重放")
    );
    let mut rows = rebound();
    rows.push(row(55, "seed-runtime", "source-generation-start", "passed"));
    rows.push(row(
        56,
        "seed-runtime",
        "source-generation-recover",
        "passed",
    ));
    rows.push(row(57, "seed-runtime", "source-export", "passed"));
    rejected(&args(rows));
}

#[test]
fn failed_export_requires_explicit_reconciliation_proof_and_current_owner() {
    let mut rows = generation();
    let origin = row(57, "seed-runtime", "source-export", "failed");
    rows.push(origin.clone());
    rows.push(row(58, "seed-runtime", "source-export-reconcile", "failed"));
    rows.push(row(
        59,
        "seed-runtime",
        "source-export-reconcile",
        "running",
    ));
    let mut input = args(rows);
    input["current"] = json!(59);
    assert_eq!(
        accepted(&input)["actions"],
        json!([
        {"kind":"export-reconcile","origin":origin},
        {"kind":"export-reconcile","origin":origin},{"kind":"owner"}])
    );
    input["attempts"][7]["mode"] = json!("arm-input");
    // 历史结构依旧可读；失败导出的实际续查证明必须由采集器拒绝错误当前模式。
    assert_eq!(accepted(&input)["actions"][0]["kind"], "export-reconcile");
}

#[test]
fn failed_start_accepts_only_zero_output_or_exact_single_owned_recovery() {
    let mut rows = rebound();
    rows.push(row(55, "seed-runtime", "source-generation-start", "failed"));
    assert_eq!(
        accepted(&args(rows.clone()))["actions"][0]["kind"],
        "zero-output"
    );
    let mut input = args(rows.clone());
    input["generation_output_exists"] = json!(true);
    rejected(&input);
    rows.push(row(
        56,
        "seed-runtime",
        "source-generation-recover",
        "running",
    ));
    let mut input = args(rows);
    input["generation_output_exists"] = json!(true);
    input["current"] = json!(56);
    assert_eq!(accepted(&input)["actions"], json!([{"kind":"owner"}]));
    input["current"] = Value::Null;
    rejected(&input);
}

#[test]
fn caches_are_only_allowed_before_rebind_and_need_verified_successor_and_owner() {
    let mut rows = prefix();
    rows.push(row(54, "cache-target", "restart", "running"));
    let mut input = args(rows.clone());
    input["current"] = json!(54);
    assert_eq!(
        accepted(&input)["actions"],
        json!([{"kind":"cache","record":rows[2],"owner":true}])
    );
    input["backend_available"] = json!(false);
    rejected(&input);
    let mut rows = rebound();
    rows.push(row(55, "cache-target", "status", "passed"));
    rejected(&args(rows));
}

#[test]
fn failed_cache_needs_later_success_or_current_recovery() {
    let mut rows = prefix();
    rows.push(row(54, "cache-target", "restart", "failed"));
    rejected(&args(rows.clone()));
    let mut input = args(rows.clone());
    input["cache_recovery"] = json!(true);
    accepted(&input);
    rows.push(row(55, "cache-target", "recover", "passed"));
    accepted(&args(rows));
}

#[test]
fn cache_successor_and_rebind_receipt_must_match_across_entire_history() {
    let mut rows = prefix();
    rows.push(row(54, "cache-target", "restart", "passed"));
    rows.push(row(55, "seed-runtime", "source-rebind", "passed"));
    let plan = accepted(&args(rows));
    assert_eq!(plan["actions"][1]["kind"], "registered-cache");
    assert_eq!(plan["actions"][2]["kind"], "rebind-cache");
    for kind in ["cache", "rebind-cache"] {
        let mut input = json!({"proofs":[{"kind":"cache","successor":{"path":"a"}},
            {"kind":kind,"successor":{"path":"a"}}]});
        assert_eq!(history::proofs(&input).unwrap()["verified"], true);
        input["proofs"][1]["successor"]["path"] = json!("b");
        assert!(history::proofs(&input).unwrap().get("rejected").is_some());
    }
}

#[test]
fn archived_segments_are_excluded_only_for_same_published_source() {
    let mut rows = rebound();
    rows.push(row(55, "seed-runtime", "stop", "passed"));
    rows.push(row(56, "storage-target", "restart", "passed"));
    let mut input = args(rows.clone());
    input["archive"] =
        json!({"receipt":{"source_registration":{"attempt":52}},"records":[rows[3]]});
    input["segment"] = json!({"records":[rows[4]]});
    accepted(&input);
    input["archive"]["receipt"]["source_registration"] = json!({"attempt":51});
    rejected(&input);
}

#[test]
fn active_reads_reject_multiple_or_mutating_stages_and_boolean_sequence() {
    assert_eq!(
        active_read(&json!({"attempts":prefix()})).unwrap()["current"],
        Value::Null
    );
    for mode in [
        "arm-input",
        "source-generation-start",
        "source-generation-stop",
        "source-generation-recover",
        "source-export",
        "source-export-reconcile",
    ] {
        let mut rows = rebound();
        rows.push(row(55, "seed-runtime", mode, "running"));
        assert_eq!(
            active_read(&json!({"attempts":rows})).unwrap()["current"],
            55
        );
    }
    let mut rows = rebound();
    rows.push(row(55, "storage-target", "restart", "running"));
    assert!(
        active_read(&json!({"attempts":rows}))
            .unwrap()
            .get("rejected")
            .is_some()
    );
}

#[test]
fn register_requires_same_run_exact_running_stage_and_no_prior_rebind_or_arm() {
    let mut rows = prefix();
    rows.push(row(54, "seed-runtime", "source-rebind", "running"));
    let mut input = json!({"attempts":rows,"number":54,"source_directory":"D:/工作区/run","run_directory":"D:/工作区/run"});
    assert_eq!(register_stage(&input).unwrap()["prefix"], json!(prefix()));
    input["run_directory"] = json!("D:/other");
    assert!(register_stage(&input).unwrap().get("rejected").is_some());
    input["run_directory"] = input["source_directory"].clone();
    input["attempts"][2]["status"] = json!("passed");
    assert!(register_stage(&input).unwrap().get("rejected").is_some());
    for mode in ["source-rebind", "arm-input"] {
        let rows = vec![
            row(54, "seed-runtime", mode, "passed"),
            row(55, "seed-runtime", "source-rebind", "running"),
        ];
        assert!(
            register_plan(&json!({"later":rows,"number":55}))
                .unwrap()
                .get("rejected")
                .is_some()
        );
    }
}

fn storage(n: u64) -> Value {
    json!({"attempt":n,"request":{"path":"D:/配置.json"},"restart_result":{"attempt":n},
        "data_directory":{"path":"D:/存储"},"api_url":"http://127.0.0.1:29000","console_url":"http://127.0.0.1:29001",
        "storage":{"sha256":"a".repeat(64),"identity":{"executable":"D:/rustfs.exe","pid":n,"started":n.to_string()}}})
}

#[test]
fn receipt_is_append_only_and_rejects_changed_source_storage_or_prefix() {
    let observed = json!({"source":{"storage":{"storage":storage(7)},"review_successor":{"source_result":{"attempt":52}}},
        "current":storage(53),"prefix":prefix()});
    let mut input = json!({"before":observed,"after":observed,"request":{"path":"D:/后继.json"}});
    let before = input.clone();
    let value = receipt(&input).unwrap();
    assert_eq!(input, before);
    assert_eq!(
        value["history_sha256"],
        digest::plan_hash(&json!(prefix())).unwrap()
    );
    assert_eq!(value["remote_writes"], 0);
    assert_eq!(value["restore_qualified"], false);
    for path in [
        "/after/current/storage/identity/started",
        "/after/source/review_successor/source_result",
        "/after/prefix/0/result",
    ] {
        *input.pointer_mut(path).unwrap() = json!("changed");
        assert!(receipt(&input).unwrap().get("rejected").is_some());
        input = before.clone();
    }
}

#[test]
fn restart_requires_published_source_and_exact_running_storage_stage() {
    let mut rows = prefix();
    rows[1]["status"] = json!("running");
    assert_eq!(
        restart_stage(&json!({"attempts":rows,"number":53})).unwrap()["descriptor"],
        json!({"attempt":52})
    );
    rows[1]["stage"] = json!("storage-source");
    assert!(
        restart_stage(&json!({"attempts":rows,"number":53}))
            .unwrap()
            .get("rejected")
            .is_some()
    );
    assert!(
        restart_stage(&json!({"attempts":[],"number":null}))
            .unwrap()
            .get("rejected")
            .is_some()
    );
}

#[test]
fn restart_accepts_only_retryable_archive_boundary_and_never_after_arm() {
    for (number, phase) in [
        (Value::Null, "stopped"),
        (Value::Null, "storage-retryable"),
        (json!(55), "storage-running"),
    ] {
        let input =
            json!({"later":[{"mode":"source-rebind"}],"segment":{"phase":phase},"number":number});
        assert_eq!(restart_boundary(&input).unwrap()["ready"], true);
    }
    for phase in ["storage-ready", "ready", "cache-running"] {
        assert!(
            restart_boundary(
                &json!({"later":[{"mode":"source-rebind"}],"segment":{"phase":phase},"number":null})
            )
            .unwrap()
            .get("rejected")
            .is_some()
        );
    }
    assert!(
        restart_boundary(&json!({"later":[{"mode":"arm-input"}],"segment":null,"number":null}))
            .unwrap()
            .get("rejected")
            .is_some()
    );
}

#[test]
fn storage_resolution_selects_only_exact_successful_rebind_and_full_prefix() {
    let rows = rebound();
    let selected = resolve_plan(&json!({"attempts":rows,"later":&rows[1..]})).unwrap();
    assert_eq!(selected["record"], rows[2]);
    assert_eq!(selected["prefix"], json!(prefix()));
    let mut bad = rows;
    bad.push(row(55, "seed-runtime", "source-rebind", "passed"));
    assert!(
        resolve_plan(&json!({"attempts":bad,"later":&bad[1..]}))
            .unwrap()
            .get("rejected")
            .is_some()
    );
    assert_eq!(
        resolve_plan(&json!({"attempts":prefix(),"later":[]})).unwrap()["record"],
        Value::Null
    );
}

#[test]
fn storage_resolution_is_offline_by_default_and_live_requires_ready_exact_generation() {
    let original = storage(7);
    let rebound = storage(53);
    let mut args = json!({"original":original,"rebound":rebound,"registered":rebound,
        "segment":null,"live":false,"current":null});
    assert_eq!(resolve_storage(&args).unwrap()["effective"], rebound);
    args["registered"] = original.clone();
    assert!(resolve_storage(&args).unwrap().get("rejected").is_some());
    args["registered"] = rebound.clone();
    args["live"] = json!(true);
    for current in [Value::Null, original, storage(54)] {
        args["current"] = current;
        assert!(resolve_storage(&args).unwrap().get("rejected").is_some());
    }
    args["current"] = rebound.clone();
    assert_eq!(resolve_storage(&args).unwrap()["effective"], rebound);
    args["segment"] = json!({"phase":"stopped","storage":null});
    assert!(resolve_storage(&args).unwrap().get("rejected").is_some());
    args["segment"] = json!({"phase":"storage-ready","storage":storage(60)});
    args["current"] = storage(60);
    assert!(resolve_storage(&args).unwrap().get("rejected").is_some());
    args["segment"]["phase"] = json!("ready");
    assert_eq!(resolve_storage(&args).unwrap()["effective"], storage(60));
}
