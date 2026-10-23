use super::*;

#[test]
fn restart_appends_generation_and_copies_physical_requests_with_previous_identity() {
    let mut runtime = FakeRuntime::new();
    runtime.execute("close", None).unwrap();
    let previous = runtime.fixture.state()["attempts"][3]["result"].clone();
    let result = runtime.execute("restart", Some(runtime.owner())).unwrap();
    assert_eq!(result["status"], "services_restarted");
    assert_eq!(runtime.fixture.history()["closed"], false);
    let generation = read(Path::new(text(&result["generation"], "path").unwrap())).unwrap();
    assert_eq!(generation["predecessor"], previous);
    let request = read(Path::new(
        text(&generation["redis"]["request"], "path").unwrap(),
    ))
    .unwrap();
    assert_eq!(request["previous_identity"]["pid"], 201);
    assert_eq!(request["previous_boot_id"], "boot-1");
    assert_eq!(request["previous_run_id"], "1".repeat(40));
    assert_eq!(
        runtime.fixture.history()["restart_predecessor"],
        Value::Null
    );
}

#[test]
fn restart_refuses_missing_predecessor_stale_owner_unknown_receipts_or_running_old_generation() {
    for mutation in ["no-predecessor", "owner", "orphan", "running"] {
        let mut runtime = FakeRuntime::new();
        if mutation != "no-predecessor" {
            runtime.execute("close", None).unwrap();
        }
        let owner = if mutation == "owner" {
            runtime.fixture.review.clone()
        } else {
            runtime.owner()
        };
        if mutation == "orphan" {
            fs::write(runtime.fixture.run.join("recovered-orphan.json"), b"{}").unwrap();
        }
        if mutation == "running" {
            runtime.observation = expected_running();
        }
        let before = runtime.fixture.state();
        runtime.events.clear();
        assert!(
            runtime.execute("restart", Some(owner)).is_err(),
            "{mutation}"
        );
        assert_eq!(runtime.fixture.state(), before);
        assert!(
            !runtime
                .events
                .iter()
                .any(|event| event == "storage-start" || event == "cache-start")
        );
    }
}

#[test]
fn failed_restart_cleans_only_known_new_tree_and_reconciles_back_to_external_site() {
    let mut runtime = FakeRuntime::new();
    runtime.external();
    runtime.execute("recover", Some(runtime.owner())).unwrap();
    runtime.fail = Some("cache-start".into());
    runtime.events.clear();
    assert!(runtime.execute("restart", Some(runtime.owner())).is_err());
    assert!(runtime.events.iter().any(|event| event == "tree-stop"));
    assert!(!runtime.events.iter().any(|event| event == "cache-stop"));
    runtime.fail = None;
    let value = runtime.execute("reconcile", Some(runtime.owner())).unwrap();
    assert_eq!(value["status"], "failed_restart_reconciled");
    let value = runtime.execute("restart", Some(runtime.owner())).unwrap();
    assert_eq!(value["status"], "services_restarted");
    assert_eq!(runtime.fixture.history()["closed"], false);
}

#[test]
fn unsupported_tool_drift_remains_rejected_by_native_history() {
    let mut runtime = FakeRuntime::new();
    runtime.execute("close", None).unwrap();
    runtime.fixture.services["cache_effective"] =
        runtime.fixture.services["requests"]["redis"].clone();
    runtime.fixture.services["cache_effective"]["port"] = json!(9999);
    runtime.execute("restart", Some(runtime.owner())).unwrap();
    assert!(history::validate(&runtime.fixture.run, &runtime.fixture.state(), true, true).is_err());
}

#[test]
fn restart_requests_use_effective_tools_but_keep_previous_runtime_identity() {
    let runtime = FakeRuntime::new();
    let mut services = runtime.fixture.services.clone();
    services["cache_effective"] = services["requests"]["redis"].clone();
    services["cache_effective"]["executable"] = json!("updated");
    let output = runtime.fixture.run.join("requests-test");
    fs::create_dir(&output).unwrap();
    let requests = super::super::restart::restart_requests(&services, &output).unwrap();
    assert_eq!(requests["redis"]["executable"], "updated");
    assert_eq!(requests["redis"]["previous_identity"]["pid"], 201);
    assert_eq!(requests["rustfs"], services["requests"]["rustfs"]);
}

#[test]
fn unknown_start_and_cleanup_results_retain_failed_generation_without_guessing_or_replay() {
    for operation in ["storage-start", "tree-read"] {
        let mut runtime = FakeRuntime::new();
        runtime.execute("close", None).unwrap();
        runtime.fail = Some(operation.into());
        runtime.events.clear();
        let error = runtime
            .execute("restart", Some(runtime.owner()))
            .unwrap_err();
        if operation == "tree-read" {
            assert_eq!(model::category(error.as_ref()), "RestartCleanupUnknown");
            assert!(runtime.new_running);
            assert!(runtime.events.iter().any(|event| event == "cache-stop"));
        } else {
            assert!(
                !runtime
                    .events
                    .iter()
                    .any(|event| event == "cache-start" || event == "tree-stop")
            );
        }
        let ledger = runtime.fixture.state();
        assert_eq!(ledger["attempts"][4]["status"], "failed");
        let starts = runtime
            .events
            .iter()
            .filter(|event| *event == "storage-start")
            .count();
        runtime.fail = None;
        assert!(runtime.execute("restart", Some(runtime.owner())).is_err());
        assert_eq!(
            runtime
                .events
                .iter()
                .filter(|event| *event == "storage-start")
                .count(),
            starts
        );
        assert_eq!(runtime.fixture.state(), ledger);
    }
}

#[test]
fn review_bound_tool_succession_is_accepted_without_changing_physical_target() {
    let mut runtime = FakeRuntime::new();
    let mut original = runtime.fixture.services["requests"]["redis"].clone();
    original["executable"] = json!("original");
    original["sha256"] = json!("a".repeat(64));
    original["distribution"] = json!("distribution");
    runtime.fixture.services["requests"]["redis"] = original.clone();
    json_io::write_replace(&runtime.fixture.run.join("redis/request.json"), &original).unwrap();
    let mut review = read(&runtime.fixture.review).unwrap();
    review["tools"] = json!({"redis_server":{"resolved_path":"updated","sha256":"b".repeat(64),"distribution":"distribution"}});
    json_io::write_replace(&runtime.fixture.review, &review).unwrap();
    runtime.execute("close", None).unwrap();
    let mut current = original;
    current["executable"] = json!("updated");
    current["sha256"] = json!("b".repeat(64));
    runtime.fixture.services["cache_effective"] = current;
    runtime.execute("restart", Some(runtime.owner())).unwrap();
    assert!(runtime.fixture.history()["restart_predecessor"].is_null());
}
