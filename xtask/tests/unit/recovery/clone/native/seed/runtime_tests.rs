use super::{fixture::Fixture, runtime_model};
use crate::recovery::clone::native::{json as document, lifecycle::state, resources::common};
use serde_json::{Value, json};
use std::{fs, path::Path};

#[test]
fn runtime_handoff_keeps_the_original_os_temporary_directory() {
    let fixture = Fixture::new();
    let temp = fixture.run.join("tmp");
    fs::create_dir(&temp).unwrap();
    let initial = fixture.run.join("seed-environment-test-initial.json");
    let api = fixture.run.join("seed-environment-test-api.json");
    let environment = json!({"APP_ENV":"test","TEMP":temp,"TMP":temp});
    document::write_new(&initial, &json!({"environment":environment})).unwrap();
    document::write_new(&api, &json!({"environment":environment})).unwrap();
    let mut post = super::history::Post {
        request: json!({"api_environment":common::binding(&api).unwrap()}),
        descriptor: Value::Null,
        runtime: fixture.old.clone(),
    };
    let manifest = json!({"target_environment":common::binding(&initial).unwrap()});
    assert_eq!(
        runtime_model::api_environment(&fixture.backend, &manifest, &post).unwrap(),
        environment
    );
    document::write_replace(&api, &json!({"environment":{"APP_ENV":"test"}})).unwrap();
    post.request["api_environment"] = common::binding(&api).unwrap();
    assert!(runtime_model::api_environment(&fixture.backend, &manifest, &post).is_err());

    let invalid = json!({"environment":{"APP_ENV":"test","TEMP":temp}});
    document::write_replace(&initial, &invalid).unwrap();
    document::write_replace(&api, &invalid).unwrap();
    post.request["api_environment"] = common::binding(&api).unwrap();
    let manifest = json!({"target_environment":common::binding(&initial).unwrap()});
    assert!(runtime_model::api_environment(&fixture.backend, &manifest, &post).is_err());
}

fn prepared() -> (Fixture, Value) {
    let mut fixture = Fixture::new();
    fixture.register();
    let runtime = fixture.run.join("seed-runtime").join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    for name in ["binaries.json", "runtime.json"] {
        document::write_new(&runtime.join(name), &json!({"fixture":true})).unwrap();
    }
    let post = document::read(&fixture.run.join("post-copy.json")).unwrap();
    let handoff = json!({"registration":common::binding(&fixture.run.join("seed-runtime.json")).unwrap(),
        "runtime":common::binding(&runtime.join("runtime.json")).unwrap(),"binaries":common::binding(&runtime.join("binaries.json")).unwrap(),
        "api_environment":post["api_environment"],
        "scope_id":"native-fixture"});
    document::write_new(&runtime.join("handoff.json"), &handoff).unwrap();
    fixture.stage("seed-runtime","prepare",Some(json!({"status":"seed_runtime_prepared","handoff":common::binding(&runtime.join("handoff.json")).unwrap()})),None);
    (fixture, handoff)
}
fn history(fixture: &Fixture, events: Value) {
    let runtime = fixture.run.join("seed-runtime").join("runtime");
    document::write_replace(
        &runtime.join("producer-history.json"),
        &json!({"format_version":1,"kind":"devex-clone-producer-history",
        "scope_id":"native-fixture","runtime_directory":runtime,"events":events}),
    )
    .unwrap();
}
fn identity(fixture: &Fixture, started: &str) -> Value {
    json!({"pid":991,"started":started,"executable":fixture.old.join("api.exe")})
}

#[test]
fn cleanup_uses_published_handoff_despite_later_failure_source_or_tool_drift() {
    let (mut fixture, _) = prepared();
    fixture.stage("seed-runtime", "start", None, Some("ValueError"));
    fs::write(fixture.run.join("identity-plan.json"), "broken later plan").unwrap();
    fs::write(fixture.run.join("historical.json"), "broken later build").unwrap();
    assert!(fixture.protocol("inputs", json!({})).is_err());
    let value = fixture.protocol("handoff", json!({})).unwrap();
    assert_eq!(
        value["runtime"],
        json!(fixture.run.join("seed-runtime").join("runtime"))
    );
    assert_eq!(value["environment"], json!({"APP_ENV":"test"}));
    let mut state = state::load(&fixture.run, false).unwrap();
    state["attempts"][0]["result"]["sha256"] = json!("0".repeat(64));
    document::write_replace(&fixture.run.join("state.json"), &state).unwrap();
    assert!(fixture.protocol("handoff", json!({})).is_ok());
}

#[test]
fn cleanup_still_rejects_changed_own_handoff_prepare_runtime_binaries_and_registration() {
    for target in [
        "seed-runtime/runtime/handoff.json",
        "seed-runtime/runtime/runtime.json",
        "seed-runtime/runtime/binaries.json",
        "seed-runtime.json",
        "results/0005.json",
        "api-environment.json",
        "target-environment.json",
    ] {
        let (fixture, _) = prepared();
        fs::write(fixture.run.join(target), "{}").unwrap();
        assert!(fixture.protocol("handoff", json!({})).is_err(), "{target}");
    }
}

#[test]
fn incomplete_duplicate_out_of_order_or_wrong_scope_history_cannot_prove_stopped() {
    let (fixture, handoff) = prepared();
    let runtime = fixture.run.join("seed-runtime").join("runtime");
    let intent = json!({"sequence":0,"event":"start-intent","role":"api","token":"one"});
    for events in [
        json!([intent]),
        json!([intent,{"sequence":1,"event":"start-intent","role":"worker","token":"one"}]),
        json!([{ "sequence":1,"event":"start-failed","role":"api","token":"one"}]),
        json!([intent,{"sequence":1,"event":"start-failed","role":"worker","token":"one"}]),
    ] {
        history(&fixture, events);
        assert!(runtime_model::stopped_with(&runtime, &handoff, &mut |_| Ok(None)).is_err());
    }
    history(&fixture, json!([]));
    assert!(
        runtime_model::stopped_with(&runtime, &json!({"scope_id":"other"}), &mut |_| Ok(None))
            .is_err()
    );
}

#[test]
fn stopped_requires_gone_or_strictly_newer_pid_and_does_not_kill_any_process() {
    let (fixture, handoff) = prepared();
    let runtime = fixture.run.join("seed-runtime").join("runtime");
    let old = identity(&fixture, "222");
    history(
        &fixture,
        json!([{"sequence":0,"event":"start-intent","role":"api","token":"one"},
        {"sequence":1,"event":"started","role":"api","token":"one","identity":old}]),
    );
    for now in [None, Some(identity(&fixture, "223"))] {
        let value = runtime_model::stopped_with(&runtime, &handoff, &mut |pid| {
            assert_eq!(pid, 991);
            Ok(now.clone())
        })
        .unwrap();
        assert_eq!(value["processes"], json!({"api":null,"worker":null}));
    }
    for now in [
        identity(&fixture, "222"),
        identity(&fixture, "221"),
        json!({"pid":992,"started":"223","executable":fixture.old.join("api.exe")}),
        json!({"pid":991,"started":"unknown","executable":"api.exe"}),
    ] {
        assert!(
            runtime_model::stopped_with(&runtime, &handoff, &mut |_| Ok(Some(now.clone())))
                .is_err()
        );
    }
    assert!(
        runtime_model::stopped_with(&runtime, &handoff, &mut |_| Err("access denied".into()))
            .is_err()
    );
}

#[test]
fn failed_spawn_closure_counts_as_stopped_but_unknown_intent_is_preserved() {
    let (fixture, handoff) = prepared();
    let runtime = fixture.run.join("seed-runtime").join("runtime");
    history(
        &fixture,
        json!([{"sequence":0,"event":"start-intent","role":"worker","token":"one"},
        {"sequence":1,"event":"start-failed","role":"worker","token":"one","error_type":"OSError"}]),
    );
    assert!(
        runtime_model::stopped_with(&runtime, &handoff, &mut |_| panic!(
            "没有已启动的 PID，不得猜测或终止进程"
        ))
        .is_ok()
    );
    history(
        &fixture,
        json!([{"sequence":0,"event":"start-intent","role":"worker","token":"one"}]),
    );
    let before = fs::read(runtime.join("producer-history.json")).unwrap();
    assert!(runtime_model::stopped_with(&runtime, &handoff, &mut |_| Ok(None)).is_err());
    assert_eq!(
        before,
        fs::read(runtime.join("producer-history.json")).unwrap()
    );
}

#[test]
fn failed_start_requires_later_exact_double_stop_and_rejects_failed_stop_or_changed_history() {
    let (mut fixture, handoff) = prepared();
    let runtime = fixture.run.join("seed-runtime").join("runtime");
    history(&fixture, json!([]));
    let empty = json!({"state":"stopped","identity":null,"ready":false});
    let result = json!({"restart_ready":true,"handoff":common::binding(&runtime.join("handoff.json")).unwrap(),
        "stopped_evidence":runtime_model::stopped(&runtime,&handoff).unwrap(),"runtime":{"operation":"stop",
            "runtime_directory":runtime,"scope_id":handoff["scope_id"],"processes":{"api":empty,"worker":empty}}});
    fixture.stage("seed-runtime", "stop", Some(result.clone()), None);
    fixture.stage("seed-runtime", "start", None, Some("ValueError"));
    let saved = state::load(&fixture.run, true).unwrap();
    let failed = saved["attempts"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert!(
        runtime_model::restart_after_stop(
            &fixture.backend,
            &fixture.run,
            &runtime,
            &handoff,
            &failed,
            999
        )
        .is_err()
    );
    fixture.stage("seed-runtime", "stop", Some(result), None);
    assert!(
        runtime_model::restart_after_stop(
            &fixture.backend,
            &fixture.run,
            &runtime,
            &handoff,
            &failed,
            999
        )
        .is_ok()
    );
    let restart = || {
        runtime_model::restart_after_stop(
            &fixture.backend,
            &fixture.run,
            &runtime,
            &handoff,
            &failed,
            999,
        )
    };
    rejects_changed_restart_receipts(&runtime, restart);
    let mut forged = failed.clone();
    forged["error_type"] = json!("another");
    assert!(
        runtime_model::restart_after_stop(
            &fixture.backend,
            &fixture.run,
            &runtime,
            &handoff,
            &forged,
            999
        )
        .is_err()
    );
    history(
        &fixture,
        json!([{"sequence":0,"event":"start-intent","role":"worker","token":"unknown"}]),
    );
    assert!(
        runtime_model::restart_after_stop(
            &fixture.backend,
            &fixture.run,
            &runtime,
            &handoff,
            &failed,
            999
        )
        .is_err()
    );
    history(&fixture, json!([]));
    fixture.stage("seed-runtime", "stop", None, Some("ValueError"));
    assert!(
        runtime_model::restart_after_stop(
            &fixture.backend,
            &fixture.run,
            &runtime,
            &handoff,
            &failed,
            999
        )
        .is_err()
    );
}

fn rejects_changed_restart_receipts(runtime: &Path, restart: impl Fn() -> crate::Result<()>) {
    let api = runtime.join("api.json");
    document::write_new(&api, &json!({"changed":true})).unwrap();
    assert!(restart().is_err());
    fs::remove_file(&api).unwrap();
    let handoff_file = runtime.join("handoff.json");
    let original = fs::read(&handoff_file).unwrap();
    document::write_replace(&handoff_file, &json!({"changed":true})).unwrap();
    assert!(restart().is_err());
    fs::write(&handoff_file, original).unwrap();
}

#[test]
fn publication_without_outer_finish_remains_unpublished_and_cannot_authorize_worker() {
    let mut fixture = Fixture::new();
    let args = fixture.begin_registration();
    fixture.protocol("register", args).unwrap();
    assert!(fixture.run.join("seed-runtime.json").is_file());
    assert!(
        fixture
            .protocol("registration", json!({"cleanup":false}))
            .is_err()
    );
    assert!(fixture.protocol("handoff", json!({})).is_err());
    assert!(state::begin(&fixture.run, "seed-runtime", "start", &json!({})).is_err());
}

#[test]
fn child_waits_for_owned_controller() {
    use std::io::Read;
    if std::env::args().any(|value| value == "--exact") {
        let _ = std::io::stdin().read(&mut [0_u8]);
    }
}

#[test]
fn actual_owned_child_crash_preserves_history_then_proves_original_generation_ended() {
    use std::process::{Command, Stdio};
    let (fixture, handoff) = prepared();
    let runtime = fixture.run.join("seed-runtime").join("runtime");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "recovery::clone::native::seed::runtime_tests::child_waits_for_owned_controller",
            "--nocapture",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let identity = common::process_identity(child.id()).unwrap().unwrap();
    history(
        &fixture,
        json!([{"sequence":0,"event":"start-intent","role":"worker","token":"crash"},
        {"sequence":1,"event":"started","role":"worker","token":"crash","identity":identity}]),
    );
    let before = fs::read(runtime.join("producer-history.json")).unwrap();
    let live = runtime_model::stopped(&runtime, &handoff);
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(live.is_err());
    assert!(runtime_model::stopped(&runtime, &handoff).is_ok());
    assert_eq!(
        before,
        fs::read(runtime.join("producer-history.json")).unwrap()
    );
}
