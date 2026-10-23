use super::super::super::{json as doc, resources::common};
use super::fixture::{Fixture, snapshot};
use serde_json::{Value, json};

#[test]
fn successor_preserves_registration_and_publishes_bound_death_proof() {
    let mut f = Fixture::new();
    f.seed();
    let registration = std::fs::read(f.directory.join("cache-target/registration.json")).unwrap();
    f.execute("restart", false).unwrap();
    let proof = f.binding().unwrap();
    assert_eq!(proof["attempt"], 3);
    assert_eq!(proof["redis"]["wsl"], f.ports.process["wsl"]);
    assert!(proof.get("tools").is_some());
    assert_eq!(
        f.status().unwrap()["processes"][0]["process"]["state"],
        "stopped"
    );
    f.execute("stop", false).unwrap();
    assert_eq!(
        std::fs::read(f.directory.join("cache-target/registration.json")).unwrap(),
        registration
    );
    assert_eq!(f.writes(), 2);
}
#[test]
fn successor_unknown_write_reconciles_without_replay() {
    let mut f = Fixture::new();
    f.seed();
    f.ports.failure = Some("after");
    assert!(f.execute("restart", false).is_err());
    assert!(
        !f.call("preflight", json!({"filename":null,"mode":"resume"}))
            .unwrap()
            .is_null()
    );
    assert!(f.execute("resume", false).is_err());
    f.ports.failure = None;
    assert!(
        !f.call("preflight", json!({"filename":null,"mode":"reconcile"}))
            .unwrap()
            .is_null()
    );
    f.execute("reconcile", false).unwrap();
    assert_eq!(f.binding().unwrap()["attempt"], 5);
    assert_eq!(f.writes(), 2);
}
#[test]
fn tampered_predecessor_death_proof_cannot_reuse_old_success() {
    let mut f = Fixture::new();
    f.seed();
    f.execute("restart", false).unwrap();
    let file = f.output(3).join("predecessor-0001.json");
    let mut value = doc::read(&file).unwrap();
    value["observation"]["state"] = json!("running");
    f.overwrite(&file, &value);
    assert!(f.binding().is_err());
}
#[test]
fn changed_successor_is_rejected_before_new_stage_or_file() {
    let mut f = Fixture::new();
    f.seed();
    f.execute("restart", false).unwrap();
    let other = f.root.path().join("other-successor.json");
    doc::write_new(
        &other,
        &json!({"kind":"devex-clone-seed-review-successor","id":"other"}),
    )
    .unwrap();
    let before = snapshot(&f.directory);
    assert!(
        f.call("preflight", json!({"filename":other,"mode":"restart"}))
            .is_err()
    );
    assert_eq!(snapshot(&f.directory), before);
    let original = f.directory.join("cache-target/request.json");
    assert!(
        f.call("preflight", json!({"filename":original,"mode":"restart"}))
            .is_err()
    );
}
#[test]
fn failure_before_start_recovers_with_registered_tools_without_stopping_old_generation() {
    let mut f = Fixture::new();
    f.seed();
    f.ports.failure = Some("predecessor");
    assert!(f.execute("restart", false).is_err());
    f.ports.failure = None;
    let stops = f.ports.stop_calls;
    assert_eq!(
        f.execute("recover", false).unwrap()["status"],
        "cache_stopped"
    );
    assert_eq!(f.ports.stop_calls, stops);
}
#[test]
fn rewritten_start_cannot_detach_death_proof_from_published_result() {
    let mut f = Fixture::new();
    f.seed();
    f.execute("restart", false).unwrap();
    let output = f.output(3);
    let file = output.join("predecessor-0001.json");
    let mut value = doc::read(&file).unwrap();
    value["observation"]["extra"] = json!("changed");
    f.overwrite(&file, &value);
    let mut start = doc::read(&output.join("start.json")).unwrap();
    start["predecessors"] = json!([common::binding(&file).unwrap()]);
    f.overwrite(&output.join("start.json"), &start);
    assert!(f.status().is_err());
    assert!(f.binding().is_err());
}
#[test]
fn frozen_successor_stays_bound_to_same_published_source_and_request() {
    let mut f = Fixture::new();
    let selected = f.seed();
    f.execute("restart", false).unwrap();
    let source = f.ports.source["review_successor"]["source_result"].clone();
    let p = json!({"owned_lock_identity":null,"before":null,"frozen":[source,selected]});
    assert!(f.call("binding", p.clone()).is_ok());
    let mut invalid = p;
    invalid["frozen"][0] = json!({"path":"other"});
    assert!(f.call("binding", invalid).is_err());
}
#[test]
fn preflight_rejects_handoff_and_arm_input_outside_unique_segment() {
    let mut f = Fixture::new();
    f.seed();
    f.execute("restart", false).unwrap();
    f.ports.boundaries = json!({"later":[{"mode":"arm-input"}],"segment":null});
    assert!(
        f.call(
            "preflight",
            json!({"filename":Value::Null,"mode":"restart"})
        )
        .is_err()
    );
}
