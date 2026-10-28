use super::*;

#[test]
fn status_is_read_only_and_reports_close_then_restart() {
    let mut runtime = FakeRuntime::new();
    let before = runtime.fixture.state();
    assert_eq!(
        runtime.execute("status", None).unwrap()["next_operation"],
        "close"
    );
    assert_eq!(runtime.fixture.state(), before);
    assert!(!runtime.fixture.run.join("run-control.guard").exists());
    runtime.execute("close", None).unwrap();
    let before = runtime.fixture.state();
    assert_eq!(
        runtime.execute("status", None).unwrap()["next_operation"],
        "restart"
    );
    assert_eq!(runtime.fixture.state(), before);
    runtime.observation = expected_running();
    assert!(runtime.execute("status", None).is_err());
    assert_eq!(runtime.fixture.state(), before);
}

#[test]
fn initial_status_never_skips_failed_or_running_stages_or_existing_evidence() {
    for count in [1, 2, 3] {
        for outcome in ["passed", "failed", "running"] {
            if count == 3 && outcome == "passed" {
                continue;
            }
            for existing in [false, true] {
                if count == 2 && outcome == "passed" && !existing {
                    // 完成双服务后的建议由完整首代 fixture 覆盖，不能只凭账本推断存活。
                    continue;
                }
                let mut runtime = FakeRuntime::new();
                let operation = if count == 1 { "redis" } else { "buckets" };
                let next = runtime.fixture.run.join(operation);
                if next.exists() {
                    assert!(next.starts_with(runtime.fixture._temporary.path()));
                    fs::remove_dir_all(&next).unwrap();
                }
                if existing {
                    fs::write(&next, b"preserved-initial-evidence").unwrap();
                }
                let mut ledger = runtime.fixture.state();
                ledger["attempts"].as_array_mut().unwrap().truncate(count);
                let last = &mut ledger["attempts"][count - 1];
                last["status"] = json!(outcome);
                if outcome != "passed" {
                    last["result"] = Value::Null;
                    last["error_type"] = if outcome == "failed" {
                        json!("ValueError")
                    } else {
                        last["finished_at"] = Value::Null;
                        Value::Null
                    };
                }
                json_io::write_replace(&runtime.fixture.run.join("state.json"), &ledger).unwrap();
                let before = snapshot(&runtime.fixture.run);
                let result = runtime.execute("status", None).unwrap();
                assert_eq!(
                    result["next_operation"],
                    if outcome == "passed" && !existing {
                        operation
                    } else {
                        "reconcile-evidence"
                    }
                );
                assert_eq!(result["services"], Value::Null);
                assert_eq!(snapshot(&runtime.fixture.run), before);
                assert!(!runtime.events.iter().any(|event| {
                    event == "registered"
                        || event == "observe"
                        || event.ends_with("-stop")
                        || event.ends_with("-start")
                }));
            }
        }
    }
}

#[test]
fn external_status_binds_owner_and_requires_recovery_before_restart() {
    let mut runtime = FakeRuntime::new();
    runtime.external();
    let before = runtime.fixture.state();
    let result = runtime.execute("status", None).unwrap();
    assert_eq!(result["next_operation"], "recover");
    assert_eq!(result["services"], "external-termination-unreconciled");
    assert_eq!(
        result["reconciliation"]["owner"],
        binding(&runtime.fixture.run.join("state.json")).unwrap()
    );
    assert_eq!(runtime.fixture.state(), before);
    runtime.execute("recover", Some(runtime.owner())).unwrap();
    let before = runtime.fixture.state();
    let result = runtime.execute("status", None).unwrap();
    assert_eq!(result["next_operation"], "restart");
    assert_eq!(result["services"], "external-termination-reconciled");
    assert_eq!(runtime.fixture.state(), before);
}

#[test]
fn controller_identity_and_unknown_receipts_take_precedence_without_service_operations() {
    for (controller, next) in [
        (
            json!({"process_matches":true,"process_missing":false}),
            Some("wait-controller"),
        ),
        (
            json!({"process_matches":false,"process_missing":true}),
            Some("recover"),
        ),
        (
            json!({"process_matches":false,"process_missing":false}),
            None,
        ),
    ] {
        let mut runtime = FakeRuntime::new();
        runtime.controller = controller;
        let before = runtime.fixture.state();
        let result = runtime.execute("status", None);
        if let Some(next) = next {
            assert_eq!(result.unwrap()["next_operation"], next);
        } else {
            assert!(result.is_err());
        }
        assert_eq!(runtime.fixture.state(), before);
        assert!(
            !runtime
                .events
                .iter()
                .any(|event| event == "registered" || event == "observe")
        );
    }
    let mut runtime = FakeRuntime::new();
    fs::write(
        runtime.fixture.run.join("recovery-orphan.intent.json"),
        b"{}",
    )
    .unwrap();
    let result = runtime.execute("status", None).unwrap();
    assert_eq!(result["next_operation"], "reconcile-evidence");
    assert!(
        !runtime
            .events
            .iter()
            .any(|event| event == "registered" || event == "observe")
    );
}

#[test]
fn failed_close_status_requires_complete_evidence_and_never_repeats_shutdown() {
    for complete in [true, false] {
        let mut runtime = FakeRuntime::new();
        runtime.fail = Some(if complete { "completion" } else { "cache-stop" }.into());
        assert!(runtime.execute("close", None).is_err());
        runtime.fail = None;
        if complete {
            runtime.stopped_members(&runtime.fixture.services["tree"]);
        }
        runtime.events.clear();
        let before = runtime.fixture.state();
        let result = runtime.execute("status", None).unwrap();
        assert_eq!(
            result["next_operation"],
            if complete {
                "reconcile"
            } else {
                "reconcile-evidence"
            }
        );
        assert_eq!(runtime.fixture.state(), before);
        assert!(
            !runtime
                .events
                .iter()
                .any(|event| event.ends_with("-stop") || event.ends_with("-start"))
        );
    }
}

pub(super) fn snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                directories.push(path);
            } else {
                files.insert(path.clone(), fs::read(path).unwrap());
            }
        }
    }
    files
}

#[test]
fn status_never_changes_any_file_or_cleans_foreign_recovery_evidence() {
    let mut runtime = FakeRuntime::new();
    for unknown in [false, true] {
        if unknown {
            fs::write(
                runtime.fixture.run.join("recovery-foreign.intent.json"),
                b"unknown-write",
            )
            .unwrap();
        }
        let before = snapshot(&runtime.fixture.run);
        runtime.execute("status", None).unwrap();
        assert_eq!(snapshot(&runtime.fixture.run), before);
    }
}
