use super::*;
use std::{collections::VecDeque, fs, path::PathBuf};
#[path = "tests/context.rs"]
mod context_tests;
#[path = "tests/observation.rs"]
mod observation_tests;
#[path = "tests/recovery.rs"]
mod recovery;
#[path = "tests/registration.rs"]
mod registration_tests;
#[path = "tests/restart.rs"]
mod restart_tests;
#[path = "tests/status.rs"]
mod status_tests;
#[path = "tests/support.rs"]
mod support;
#[path = "tests/wait.rs"]
mod wait;
use support::*;

#[test]
fn write_and_owner_policy_reject_before_context_or_service_access() {
    for operation in ["close", "recover", "reconcile", "restart"] {
        let mut runtime = FakeRuntime::new();
        let fixture = &runtime.fixture;
        let (backend, review, bootstrap) = (
            fixture.backend.clone(),
            fixture.review.clone(),
            fixture.bootstrap.clone(),
        );
        assert!(
            execute_with(
                &mut runtime,
                &backend,
                operation,
                &review,
                &bootstrap,
                None,
                false
            )
            .is_err()
        );
        assert!(runtime.events.is_empty());
    }
}

#[test]
fn close_stops_redis_before_tree_and_appends_only_one_successful_generation() {
    let mut runtime = FakeRuntime::new();
    let prefix = state::attempts(&runtime.fixture.state()).unwrap().to_vec();
    let value = runtime.execute("close", None).unwrap();
    assert_eq!(value["status"], "services_closed");
    assert!(runtime.event_position("cache-stop") < runtime.event_position("tree-stop"));
    assert_eq!(
        &state::attempts(&runtime.fixture.state()).unwrap()[..3],
        &prefix
    );
    assert_eq!(runtime.fixture.history()["closed"], true);
    assert!(!runtime.fixture.run.join("run.lock").exists());
    let before = fs::read(runtime.fixture.run.join("state.json")).unwrap();
    let value = runtime.execute("close", None).unwrap();
    assert_eq!(value["status"], "services_already_closed");
    assert_eq!(
        fs::read(runtime.fixture.run.join("state.json")).unwrap(),
        before
    );
}

#[test]
fn unknown_redis_stop_is_not_replayed_and_never_stops_the_tree() {
    let mut runtime = FakeRuntime::new();
    runtime.fail = Some("cache-stop".into());
    assert!(runtime.execute("close", None).is_err());
    assert!(!runtime.events.iter().any(|event| event == "tree-stop"));
    assert_eq!(runtime.fixture.state()["attempts"][3]["status"], "failed");
    let count = runtime
        .events
        .iter()
        .filter(|event| *event == "cache-stop")
        .count();
    assert!(runtime.execute("close", None).is_err());
    assert_eq!(
        runtime
            .events
            .iter()
            .filter(|event| *event == "cache-stop")
            .count(),
        count
    );
}

#[test]
fn close_refuses_unknown_recovery_and_controller_pid_reuse_before_writes() {
    let mut runtime = FakeRuntime::new();
    fs::write(
        runtime.fixture.run.join("recovery-orphan.intent.json"),
        b"{}",
    )
    .unwrap();
    let before = runtime.fixture.state();
    assert!(runtime.execute("close", None).is_err());
    assert_eq!(runtime.fixture.state(), before);
    assert!(!runtime.events.iter().any(|event| event == "cache-stop"));
    fs::remove_file(runtime.fixture.run.join("recovery-orphan.intent.json")).unwrap();
    runtime.controller = json!({"owner":{},"process_matches":false,"process_missing":false});
    assert!(runtime.execute("close", None).is_err());
    assert_eq!(runtime.fixture.state(), before);
}

#[test]
fn input_drift_during_redis_stop_blocks_tree_and_retains_failed_attempt() {
    let mut runtime = FakeRuntime::new();
    runtime.drift_on_stop = true;
    assert!(runtime.execute("close", None).is_err());
    assert!(!runtime.events.iter().any(|event| event == "tree-stop"));
    assert_eq!(runtime.fixture.state()["attempts"][3]["status"], "failed");
}

#[test]
fn close_rejects_external_termination_without_inventing_normal_shutdown() {
    let mut runtime = FakeRuntime::new();
    runtime.external();
    let before = runtime.fixture.state();
    assert!(runtime.execute("close", None).is_err());
    assert_eq!(runtime.fixture.state(), before);
    assert!(
        !runtime
            .events
            .iter()
            .any(|event| event == "tree-stop" || event == "cache-stop")
    );
}

#[test]
fn context_guard_rejects_every_bound_document_or_private_environment_change() {
    for key in ["review", "bootstrap", "manifest", "environment"] {
        let runtime = FakeRuntime::new();
        let context = Context::load(&runtime.fixture.backend, runtime.fixture.context()).unwrap();
        let path = Path::new(text(&context.value["sources"][key], "path").unwrap());
        fs::write(path, b"{}").unwrap();
        assert!(context.guard(None).is_err());
    }
}
