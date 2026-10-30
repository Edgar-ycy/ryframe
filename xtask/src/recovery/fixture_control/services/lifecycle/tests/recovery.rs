use super::*;

fn failed_close(runtime: &mut FakeRuntime) {
    runtime.fail = Some("completion".into());
    assert!(runtime.execute("close", None).is_err());
    runtime.fail = None;
    runtime.stopped_members(&runtime.fixture.services["tree"]);
}

#[test]
fn failed_close_reconcile_appends_evidence_without_service_operations() {
    let mut runtime = FakeRuntime::new();
    failed_close(&mut runtime);
    runtime.events.clear();
    let value = runtime.execute("reconcile", Some(runtime.owner())).unwrap();
    assert_eq!(value["status"], "failed_close_reconciled");
    assert_eq!(runtime.fixture.history()["closed"], true);
    assert!(!runtime.events.iter().any(|event| {
        ["cache-stop", "tree-stop", "cache-start", "storage-start"].contains(&event.as_str())
    }));
}

#[test]
fn failed_close_reconcile_rejects_stale_owner_tampered_members_extra_files_and_controller() {
    for mutation in ["owner", "members", "extra", "controller", "failed-sources"] {
        let mut runtime = FakeRuntime::new();
        failed_close(&mut runtime);
        let mut owner = runtime.owner();
        match mutation {
            "owner" => {
                owner = runtime.fixture.review.clone();
            }
            "members" => {
                let proof = runtime
                    .fixture
                    .run
                    .join("rustfs")
                    .join(format!("rustfs-members-{}-stopped.json", "b".repeat(32)));
                let mut value = read(&proof).unwrap();
                value["error_type"] = json!("unexpected");
                json_io::write_replace(&proof, &value).unwrap();
            }
            "extra" => {
                fs::write(
                    runtime
                        .fixture
                        .run
                        .join("lifecycle-0004")
                        .join("unknown.json"),
                    b"{}",
                )
                .unwrap();
            }
            "controller" => {
                let path = state::controller_path(&runtime.fixture.run, 4);
                let mut value = read(&path).unwrap();
                value["attempt_sha256"] = json!("0".repeat(64));
                json_io::write_replace(&path, &value).unwrap();
            }
            "failed-sources" => {
                let mut value = runtime.fixture.state();
                value["attempts"][3]["sources"]["invented"] = json!({});
                json_io::write_replace(&runtime.fixture.run.join("state.json"), &value).unwrap();
            }
            _ => unreachable!(),
        }
        let before = fs::read(runtime.fixture.run.join("state.json")).unwrap();
        assert!(
            runtime.execute("reconcile", Some(owner)).is_err(),
            "{mutation}"
        );
        assert_eq!(
            fs::read(runtime.fixture.run.join("state.json")).unwrap(),
            before,
            "{mutation}"
        );
    }
}

#[test]
fn reconciliation_history_rejects_rebound_controller_and_wrong_result_owner() {
    for key in ["proof-controller", "result-owner"] {
        let mut runtime = FakeRuntime::new();
        failed_close(&mut runtime);
        runtime.execute("reconcile", Some(runtime.owner())).unwrap();
        let result_file = state::result_path(&runtime.fixture.run, 5);
        let mut result = read(&result_file).unwrap();
        if key == "proof-controller" {
            let proof_file = PathBuf::from(text(&result["reconciliation"], "path").unwrap());
            let mut proof = read(&proof_file).unwrap();
            proof["failed_controller"] =
                binding(&state::controller_path(&runtime.fixture.run, 5)).unwrap();
            json_io::write_replace(&proof_file, &proof).unwrap();
            result["reconciliation"] = binding(&proof_file).unwrap();
        } else {
            result["owner"] = result["controller"].clone();
        }
        json_io::write_replace(&result_file, &result).unwrap();
        let mut value = read(&runtime.fixture.run.join("state.json")).unwrap();
        value["attempts"][4]["result"] = binding(&result_file).unwrap();
        json_io::write_replace(&runtime.fixture.run.join("state.json"), &value).unwrap();
        assert!(
            history::validate(&runtime.fixture.run, &runtime.fixture.state(), true, true).is_err(),
            "{key}"
        );
    }
}

#[test]
fn external_recovery_records_only_observation_and_never_starts_or_stops_services() {
    let mut runtime = FakeRuntime::new();
    runtime.external();
    let result = runtime.execute("recover", Some(runtime.owner())).unwrap();
    assert_eq!(result["status"], "external_termination_reconciled");
    assert_eq!(result["next_operation"], "restart");
    assert!(!runtime.events.iter().any(|event| {
        ["cache-stop", "tree-stop", "cache-start", "storage-start"].contains(&event.as_str())
    }));
    let proof = read(Path::new(text(&result["evidence"], "path").unwrap())).unwrap();
    assert!(proof["normal_shutdown_proof"].is_null());
    assert_eq!(
        runtime.fixture.history()["external_recovery"],
        runtime.fixture.state()["attempts"][3]["result"]
    );
    assert!(runtime.execute("recover", Some(runtime.owner())).is_err());
}

#[test]
fn external_recovery_rejects_live_or_changing_services_before_publication() {
    let mut runtime = FakeRuntime::new();
    let before = runtime.fixture.state();
    assert!(runtime.execute("recover", Some(runtime.owner())).is_err());
    assert_eq!(runtime.fixture.state(), before);
    runtime.external();
    runtime.drift_observe_at = Some(runtime.observes + 2);
    assert!(runtime.execute("recover", Some(runtime.owner())).is_err());
    assert_eq!(runtime.fixture.state(), before);
}

#[test]
fn controller_recovery_requires_dead_exact_owner_and_publishes_one_receipt() {
    for variant in 0..3 {
        recover_legacy_ledger(variant);
    }
}

fn recover_legacy_ledger(variant: usize) {
    let mut runtime = FakeRuntime::new();
    let prefix = legacy_close(&mut runtime, variant);
    let context = Context::load(&runtime.fixture.backend, runtime.fixture.context()).unwrap();
    let history = context.history(true).unwrap();
    context.require_current_owner(&runtime.owner()).unwrap();
    assert_eq!(history["closed"], true);
    assert!(!history["restart_predecessor"].is_null());
    assert!(history["unsettled"].as_array().unwrap().is_empty());
    context.no_unknown().unwrap();
    assert!(controller(&mut runtime, &context).unwrap().is_null());
    let previous = registered(&mut runtime, &context, true).unwrap();
    check_observation(
        &mut runtime,
        &previous,
        &expected_stopped(),
        "重启前服务未停止",
    )
    .unwrap();
    let run = runtime.fixture.run.clone();
    let guard = ProcessGuard::acquire(&run).unwrap();
    let lock = RunLock::claim(&run).unwrap();
    context.guard(None).unwrap();
    check_observation(
        &mut runtime,
        &previous,
        &expected_stopped(),
        "持锁后服务状态变化",
    )
    .unwrap();
    let mut sources = context.sources();
    sources["restart_predecessor"] = history["restart_predecessor"].clone();
    let mut dead = lock.owner.clone();
    dead["identity"]["pid"] = json!(2147481000u32);
    assert!(
        crate::recovery::clone::native::lifecycle::identity::process(2147481000)
            .unwrap()
            .is_none()
    );
    json_io::write_replace(&run.join("run.lock/owner.json"), &dead).unwrap();
    // 对应正式 restart 的 stage 已绑定控制器、尚未进入 body 的中断窗口。
    let (number, attempt) = ledger::begin(&run, "restart", &sources).unwrap();
    state::bind_controller(&run, number, &dead, &attempt).unwrap();
    let descriptor = binding(&state::controller_path(&run, number)).unwrap();
    let before = binding(&run.join("state.json")).unwrap();
    assert!(!run.join(format!("lifecycle-{number:04}")).exists());
    drop(guard);
    history::validate(&run, &runtime.fixture.state(), true, false).unwrap();
    let selected = run.parent().unwrap().join("owner-binding.json");
    json_io::write_new(&selected, &descriptor).unwrap();
    runtime.controller = json!({"owner":descriptor,"process_missing":true,"process_matches":false});
    runtime.events.clear();
    let value = runtime.execute("recover", Some(selected.clone())).unwrap();
    assert_eq!(value["status"], "controller_recovered");
    assert_eq!(value["next_operation"], "reconcile-evidence");
    assert!(!runtime.fixture.run.join("run.lock").exists());
    assert_recovery_publication(&runtime, &value, &descriptor, &before, &dead);
    let run = &runtime.fixture.run;
    assert_eq!(
        runtime.fixture.state()["attempts"][3]["sources"]["state_before"],
        prefix
    );
    let history = history::validate(run, &runtime.fixture.state(), true, false)
        .unwrap_or_else(|error| panic!("恢复后历史重读 variant={variant}: {error}"));
    assert_eq!(history["unsettled"], json!([5]));
    assert!(history["external_recovery"].is_null());
    let context = Context::load(&runtime.fixture.backend, runtime.fixture.context()).unwrap();
    assert!(context.unknown().unwrap().is_empty());
    let snapshot = fs::read(run.join("state.json")).unwrap();
    assert!(controller_recovery::run(&runtime.fixture.backend, run, &descriptor).is_err());
    assert!(runtime.execute("recover", Some(selected)).is_err());
    assert_eq!(
        fs::read(runtime.fixture.run.join("state.json")).unwrap(),
        snapshot
    );
    assert_eq!(
        runtime
            .events
            .iter()
            .filter(|event| *event == "recover-controller")
            .count(),
        1
    );
    assert!(!runtime.events.iter().any(|event| {
        ["cache-stop", "tree-stop", "cache-start", "storage-start"].contains(&event.as_str())
    }));
}

fn assert_recovery_publication(
    runtime: &FakeRuntime,
    value: &Value,
    descriptor: &Value,
    before: &Value,
    dead: &Value,
) {
    let run = &runtime.fixture.run;
    let recovered = read(Path::new(text(&value["recovery"], "path").unwrap())).unwrap();
    json_io::exact(
        &recovered,
        &[
            "status",
            "owner",
            "remote_writes",
            "copy_requires_reconciliation",
            "state_sha256",
        ],
        "恢复结果",
    )
    .unwrap();
    assert_eq!(recovered["owner"], *dead);
    assert_eq!(recovered["remote_writes"], 0);
    assert_eq!(recovered["copy_requires_reconciliation"], true);
    let filename = Path::new(text(&value["recovery"], "path").unwrap())
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    let id = filename
        .strip_prefix("recovered-")
        .unwrap()
        .strip_suffix(".json")
        .unwrap();
    let intent = read(&run.join(format!("recovery-{id}.intent.json"))).unwrap();
    json_io::exact(
        &intent,
        &["status", "owner", "state_before", "state_after_sha256"],
        "恢复意图",
    )
    .unwrap();
    assert_eq!(intent["status"], "controller_recovery_started");
    assert_eq!(intent["owner"], *descriptor);
    assert_eq!(intent["state_before"], *before);
    assert_eq!(intent["state_after_sha256"], recovered["state_sha256"]);
    let saved = runtime.fixture.state();
    assert!(
        saved["attempts"].as_array().unwrap()[..4]
            .iter()
            .all(|item| item["status"] == "passed")
    );
    assert_eq!(saved["attempts"][4]["status"], "failed");
    assert_eq!(saved["attempts"][4]["error_type"], "ControllerInterrupted");
    let mut interrupted = saved.clone();
    interrupted["attempts"].as_array_mut().unwrap().truncate(5);
    assert_eq!(
        crate::recovery::clone::native::digest::plan_hash(&interrupted).unwrap(),
        recovered["state_sha256"]
    );
    assert_eq!(
        fs::read_dir(run)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("recovered-")
            })
            .count(),
        1
    );
}

fn legacy_close(runtime: &mut FakeRuntime, variant: usize) -> Value {
    let run = &runtime.fixture.run;
    let mut saved = runtime.fixture.state();
    saved["attempts"][0]["sources"] = json!({"说明":"旧账本中文🌏"});
    let attempts = saved["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            let fields = [
                "number",
                "stage",
                "mode",
                "started_at",
                "finished_at",
                "status",
                "sources",
                "result",
                "error_type",
            ];
            format!(
                "{{{}}}",
                fields
                    .iter()
                    .map(|key| format!("\"{key}\":{}", item[*key]))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let ordered = format!(
        "{{\"format_version\":1,\"manifest\":{},\"attempts\":[{attempts}]}}",
        saved["manifest"]
    );
    let bytes = state::prefix::serializations(&ordered).remove(variant);
    assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), saved);
    fs::write(run.join("state.json"), bytes).unwrap();
    let prefix = binding(&run.join("state.json")).unwrap();
    runtime.execute("close", None).unwrap();
    assert!(runtime.fixture.history()["closed"].as_bool().unwrap());
    let current = fs::read(runtime.fixture.run.join("state.json")).unwrap();
    let historical = state::historical(&runtime.fixture.run, &prefix, true)
        .expect("通用fresh历史必须消费原初代写入器到原生close的精确前像");
    assert_eq!(historical["historical_attempts"], 3);
    assert_eq!(
        fs::read(runtime.fixture.run.join("state.json")).unwrap(),
        current
    );
    prefix
}

#[test]
fn live_or_reused_controller_is_rejected_without_mutating_state() {
    for (matches, missing) in [(true, false), (false, false)] {
        let mut runtime = FakeRuntime::new();
        let owner = json!({"path":"owner"});
        let selected = runtime
            .fixture
            .run
            .parent()
            .unwrap()
            .join("owner-binding.json");
        json_io::write_new(&selected, &owner).unwrap();
        runtime.controller =
            json!({"owner":owner,"process_missing":missing,"process_matches":matches});
        let before = runtime.fixture.state();
        assert!(runtime.execute("recover", Some(selected)).is_err());
        assert_eq!(runtime.fixture.state(), before);
        assert!(
            !runtime
                .events
                .iter()
                .any(|event| event == "recover-controller")
        );
    }
}

#[test]
fn complete_unpublished_close_wrapper_is_consumed_without_repeating_shutdown() {
    let mut runtime = FakeRuntime::new();
    failed_close(&mut runtime);
    let tree = runtime.fixture.services["tree"].clone();
    let members = runtime.stopped_members(&tree);
    let wrapper = runtime
        .fixture
        .run
        .join("lifecycle-0004")
        .join("rustfs-stopped.json");
    json_io::write_new(
        &wrapper,
        &json!({"tree":tree,"terminated":true,"state":"stopped","completion":members}),
    )
    .unwrap();
    let result = runtime.execute("reconcile", Some(runtime.owner())).unwrap();
    let proof = read(Path::new(text(&result["reconciliation"], "path").unwrap())).unwrap();
    assert_eq!(
        proof["evidence"]["rustfs_close"],
        binding(&wrapper).unwrap()
    );
    assert_eq!(runtime.fixture.history()["closed"], true);
}

#[test]
fn grouped_attempt_and_controller_tampering_cannot_forge_current_sources() {
    let mut runtime = FakeRuntime::new();
    failed_close(&mut runtime);
    let mut ledger = runtime.fixture.state();
    ledger["attempts"][3]["sources"]["invented"] = binding(&runtime.fixture.review).unwrap();
    let mut running = ledger["attempts"][3].clone();
    for key in ["finished_at", "result", "error_type"] {
        running[key] = Value::Null;
    }
    running["status"] = json!("running");
    let path = state::controller_path(&runtime.fixture.run, 4);
    let mut controller = read(&path).unwrap();
    controller["attempt_sha256"] =
        json!(crate::recovery::clone::native::digest::plan_hash(&running).unwrap());
    json_io::write_replace(&path, &controller).unwrap();
    json_io::write_replace(&runtime.fixture.run.join("state.json"), &ledger).unwrap();
    assert!(runtime.execute("reconcile", Some(runtime.owner())).is_err());
    assert_eq!(runtime.fixture.state(), ledger);
}

#[test]
fn external_recovery_proof_tampering_and_additional_failed_close_are_not_reconciled() {
    let mut runtime = FakeRuntime::new();
    runtime.external();
    let result = runtime.execute("recover", Some(runtime.owner())).unwrap();
    fs::write(text(&result["evidence"], "path").unwrap(), b"{}").unwrap();
    assert!(history::validate(&runtime.fixture.run, &runtime.fixture.state(), true, true).is_err());
    let mut runtime = FakeRuntime::new();
    failed_close(&mut runtime);
    let (number, _) = state::begin(
        &runtime.fixture.run,
        "fixture-services",
        "close",
        &runtime.fixture.context()["sources"],
    )
    .unwrap();
    state::finish(&runtime.fixture.run, number, None, Some("Injected")).unwrap();
    let before = runtime.fixture.state();
    assert!(runtime.execute("reconcile", Some(runtime.owner())).is_err());
    assert_eq!(runtime.fixture.state(), before);
}
