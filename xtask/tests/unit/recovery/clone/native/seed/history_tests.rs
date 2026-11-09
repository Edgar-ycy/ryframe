use super::{
    capacity::orchestration::{Prepared, before_identities},
    fixture::Fixture,
    history,
};
use crate::recovery::clone::native::{
    digest, json as document, lifecycle::state, resources::common,
};
use serde_json::{Value, json};
use std::fs;

fn amended() -> (Fixture, Value) {
    let mut fixture = Fixture::new();
    let original = history::post(&fixture.backend, &fixture.run, None, None, false).unwrap();
    let prepared = fixture.stage("post-copy", "prepare", Some(json!({"fixture":true})), None);
    let failed_number = state::begin(&fixture.run, "runtime-target", "start", &json!({}))
        .unwrap()
        .0;
    document::write_new(
        &state::controller_path(&fixture.run, failed_number),
        &json!({"fixture":true}),
    )
    .unwrap();
    document::write_new(
        &fixture.run.join(format!("failure-{failed_number:04}.json")),
        &json!({"fixture":true}),
    )
    .unwrap();
    state::finish(&fixture.run, failed_number, None, Some("ValueError")).unwrap();
    let stopped = fixture.stage(
        "runtime-target",
        "stop",
        Some(json!({"fixture":true})),
        None,
    );
    let stopped_number = state::load(&fixture.run, true).unwrap()["attempts"]
        .as_array()
        .unwrap()
        .len();
    for name in ["binaries.json", "producer-history.json"] {
        document::write_new(&fixture.old.join(name), &json!({"fixture":true})).unwrap();
    }
    let file = |name| common::binding(&fixture.old.join(name)).unwrap();
    let invalidated = json!({"prepare":prepared,"handoff":file("handoff.json"),"runtime":file("runtime.json"),
        "binaries":file("binaries.json"),"api_process":file("api.json"),"producer_history":file("producer-history.json"),
        "failed_start":{"attempt":failed_number,"controller":common::binding(&state::controller_path(&fixture.run,failed_number)).unwrap(),
            "failure":common::binding(&fixture.run.join(format!("failure-{failed_number:04}.json"))).unwrap()},
        "stopped":{"attempt":stopped_number,"result":stopped}});
    let environment = fixture.file(
        "amended-environment",
        &json!({"environment":{"APP_ENV":"test"}}),
    );
    let mut request = original.request.clone();
    request["api_environment"] = environment.clone();
    let amendment = json!({"format_version":1,"kind":"devex-clone-post-copy-amendment","sequence":1,
        "run_manifest":state::manifest_binding(&fixture.run).unwrap(),"predecessor":original.descriptor,
        "request_sha256":digest::plan_hash(&request).unwrap(),"api_environment":environment,"invalidated_runtime":invalidated});
    fs::create_dir_all(fixture.run.join("post-copy/amendments")).unwrap();
    let path = fixture.run.join("post-copy/amendments/0001.json");
    document::write_new(&path, &amendment).unwrap();
    let descriptor = common::binding(&path).unwrap();
    fixture.stage("post-copy","amend",Some(json!({"status":"post_copy_amended","registration":descriptor,
        "sequence":1,"predecessor":original.descriptor,"runtime_directory":fixture.run.join("post-copy").join("runtime-0001"),
        "remote_writes":0,"worker_must_remain_stopped":true,"restore_qualified":false})),None);
    (fixture, descriptor)
}

#[test]
fn current_post_amendment_reconstructs_request_and_cleanup_can_select_original_generation() {
    let (fixture, descriptor) = amended();
    let current = history::post(&fixture.backend, &fixture.run, None, None, false).unwrap();
    assert_eq!(current.descriptor, descriptor);
    assert_eq!(
        current.runtime,
        fixture.run.join("post-copy").join("runtime-0001")
    );
    assert_ne!(
        current.request["api_environment"],
        document::read(&fixture.run.join("post-copy.json")).unwrap()["api_environment"]
    );
    assert!(
        history::post(
            &fixture.backend,
            &fixture.run,
            Some(&fixture.request["post_copy"]),
            None,
            false
        )
        .is_err()
    );
    assert_eq!(
        history::post(
            &fixture.backend,
            &fixture.run,
            Some(&fixture.request["post_copy"]),
            None,
            true
        )
        .unwrap()
        .runtime,
        fixture.old
    );
    fs::write(fixture.old.join("binaries.json"), "changed after stopped").unwrap();
    assert!(history::post(&fixture.backend, &fixture.run, None, None, false).is_err());
    assert!(
        history::post(
            &fixture.backend,
            &fixture.run,
            Some(&descriptor),
            None,
            true
        )
        .is_ok()
    );
}

#[test]
fn quota_stage_binds_the_effective_amended_request_and_its_exact_descriptor() {
    let (fixture, descriptor) = amended();
    let current = history::post(&fixture.backend, &fixture.run, None, None, false).unwrap();
    let mut request = fixture.request.clone();
    request["post_copy"] = descriptor.clone();
    fixture.file("seed-runtime", &request);
    let (number, _) =
        state::begin(&fixture.run, "seed-runtime", "quotas-plan", &json!({})).unwrap();
    let output = fixture
        .run
        .join(format!("seed-runtime/attempt-{number:04}"));
    fs::create_dir_all(&output).unwrap();
    let accepts = |post: &Value| {
        Prepared::load(
            &fixture.backend,
            &fixture.run,
            &output,
            number,
            post,
            &request,
            "quotas-plan",
        )
        .is_ok()
    };
    assert!(accepts(&current.request));
    let raw = common::read_bound(&fixture.backend, &descriptor, "修订登记").unwrap();
    assert!(!accepts(&raw));
    let original = document::read(&fixture.run.join("post-copy.json")).unwrap();
    assert!(!accepts(&original));
}

#[test]
fn every_identity_session_artifact_blocks_quota_even_when_its_stage_failed() {
    let mut fixture = Fixture::new();
    for mode in ["identities-apply", "identities-verify"] {
        fixture.stage("seed-runtime", mode, None, Some("Interrupted"));
        let saved = state::load(&fixture.run, true).unwrap();
        let number = state::attempts(&saved).unwrap().last().unwrap()["number"]
            .as_u64()
            .unwrap();
        let output = fixture
            .run
            .join(format!("seed-runtime/attempt-{number:04}"));
        fs::create_dir_all(&output).unwrap();
        let check = || before_identities(&fixture.backend, &fixture.run, &fixture.request, &saved);
        assert!(check().is_ok());
        for name in [
            "session-launch.json",
            "session-process.json",
            "session.stdout.log",
            "session.stderr.log",
        ] {
            let path = output.join(name);
            fs::write(&path, b"").unwrap();
            assert!(check().is_err(), "{mode}: {name}");
            fs::remove_file(&path).unwrap();
        }
    }
}

#[test]
fn changed_amendment_sequence_predecessor_request_hash_or_original_file_cannot_be_adopted() {
    for field in [
        "sequence",
        "predecessor",
        "request_sha256",
        "run_manifest",
        "kind",
    ] {
        let (fixture, _) = amended();
        let path = fixture.run.join("post-copy/amendments/0001.json");
        let mut value = document::read(&path).unwrap();
        value[field] = Value::Null;
        document::write_replace(&path, &value).unwrap();
        assert!(
            history::post(&fixture.backend, &fixture.run, None, None, true).is_err(),
            "{field}"
        );
    }
}
