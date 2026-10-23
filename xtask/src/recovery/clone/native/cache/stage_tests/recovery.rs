use super::super::super::{json as doc, resources::common};
use super::fixture::{Fixture, before, snapshot};
use serde_json::json;

#[test]
fn restart_preserves_inputs_and_requires_outer_publication() {
    let mut f = Fixture::new();
    let original = std::fs::read(&f.request_file).unwrap();
    f.execute("restart", true).unwrap();
    assert!(f.binding().is_err());
    f.execute("reconcile", false).unwrap();
    let observed = f.ports.observe_calls;
    f.ports.guard_failure = true;
    assert_eq!(f.binding().unwrap()["attempt"], 2);
    assert_eq!(f.ports.observe_calls, observed);
    assert_eq!(f.writes(), 1);
    assert_eq!(std::fs::read(&f.request_file).unwrap(), original);
}
#[test]
fn resume_before_first_scan_retains_empty_owner_and_same_runtime() {
    let mut f = Fixture::new();
    f.ports.failure = Some("scan");
    assert!(f.execute("restart", false).is_err());
    assert_eq!(
        std::fs::read_dir(f.output(1).join("owner"))
            .unwrap()
            .count(),
        0
    );
    assert!(f.ports.commands.is_empty());
    let runtime = common::binding(&f.output(1).join("runtime.json")).unwrap();
    f.ports.failure = None;
    f.execute("resume", false).unwrap();
    assert_eq!(
        doc::read(&f.output(2).join("owner/intent.json")).unwrap()["runtime"],
        runtime
    );
    assert_eq!(f.ports.counter, 1);
    assert_eq!(f.writes(), 1);
}
#[test]
fn unknown_after_response_only_reconciles_without_replay() {
    let mut f = Fixture::new();
    f.ports.failure = Some("after");
    assert!(f.execute("restart", false).is_err());
    assert!(f.execute("resume", false).is_err());
    f.ports.failure = None;
    f.execute("reconcile", false).unwrap();
    assert_eq!(f.binding().unwrap()["attempt"], 3);
    assert_eq!(f.writes(), 1);
}
#[test]
fn unknown_before_requires_reconcile_then_explicit_resume() {
    let mut f = Fixture::new();
    f.ports.failure = Some("before");
    assert!(f.execute("restart", false).is_err());
    f.ports.failure = None;
    assert!(f.execute("resume", false).is_err());
    assert_eq!(
        f.execute("reconcile", false).unwrap()["status"],
        "cache_reconciled_before"
    );
    assert!(f.binding().is_err());
    f.execute("resume", false).unwrap();
    assert_eq!(f.binding().unwrap()["attempt"], 4);
    f.execute("stop", false).unwrap();
    f.execute("restart", false).unwrap();
    assert_eq!(f.writes(), 3);
}
#[test]
fn partial_namespace_reconciliation_blocks_all_new_writes() {
    let mut f = Fixture::new();
    f.ports.failure = Some("before");
    assert!(f.execute("restart", false).is_err());
    f.ports.store.insert(
        f.request["markers"]["ownership_key"]
            .as_str()
            .unwrap()
            .into(),
        f.request["markers"]["ownership_value"].clone(),
    );
    f.ports.failure = None;
    assert!(f.execute("reconcile", false).is_err());
    assert!(f.execute("resume", false).is_err());
    assert_eq!(f.writes(), 1);
}
#[test]
fn duplicate_restart_and_stop_never_fall_back_to_old_success() {
    let mut f = Fixture::new();
    f.execute("restart", false).unwrap();
    assert!(f.execute("restart", false).is_err());
    assert!(f.binding().is_err());
    f.execute("stop", false).unwrap();
    assert!(f.binding().is_err());
    f.execute("restart", false).unwrap();
    assert_eq!(f.binding().unwrap()["attempt"], 4);
}
#[test]
fn status_is_readonly_and_cleanup_avoids_current_product_configuration() {
    let mut f = Fixture::new();
    f.execute("restart", false).unwrap();
    let before = snapshot(&f.directory);
    assert_eq!(
        f.status().unwrap()["processes"][0]["process"]["state"],
        "running"
    );
    assert_eq!(snapshot(&f.directory), before);
    f.ports.guard_failure = true;
    f.ports.validate_failure = true;
    f.execute("recover", false).unwrap();
    assert!(!f.ports.alive);
}
#[test]
fn guard_failure_prevents_start_and_all_marker_writes() {
    let mut f = Fixture::new();
    f.ports.guard_failure = true;
    assert!(f.execute("restart", false).is_err());
    assert_eq!(f.writes(), 0);
    assert!(!f.ports.alive);
}
#[test]
fn cross_attempt_owner_observation_is_rejected() {
    let mut f = Fixture::new();
    f.ports.failure = Some("before");
    assert!(f.execute("restart", false).is_err());
    f.ports.failure = None;
    f.execute("reconcile", false).unwrap();
    let file = f.root.path().join("foreign-before.json");
    doc::write_new(&file, &before()).unwrap();
    let proof = f.output(1).join("owner/reconcile-0002.json");
    let mut value = doc::read(&proof).unwrap();
    value["observed"] = common::binding(&file).unwrap();
    f.overwrite(&proof, &value);
    assert!(f.execute("resume", false).is_err());
}
#[test]
fn invalid_confirmation_does_not_hide_earlier_unknown_generation() {
    let mut f = Fixture::new();
    f.execute("restart", false).unwrap();
    f.overwrite(&f.output(1).join("owner/confirmed.json"), &json!({}));
    f.execute("stop", false).unwrap();
    assert!(f.execute("restart", false).is_err());
    assert_eq!(f.ports.counter, 1);
}
#[test]
fn precisely_cleaned_partial_start_only_allows_new_restart() {
    let mut f = Fixture::new();
    f.ports.partial = true;
    assert!(f.execute("restart", false).is_err());
    f.ports.partial = false;
    assert!(f.execute("resume", false).is_err());
    f.execute("restart", false).unwrap();
    assert_eq!(f.binding().unwrap()["attempt"], 3);
}
#[test]
fn scan_rejects_outside_keys_cursor_cycles_and_invalid_shapes() {
    for failure in ["outside", "cursor", "scan-shape"] {
        let mut f = Fixture::new();
        f.ports.failure = Some(failure);
        assert!(f.execute("restart", false).is_err());
        assert_eq!(f.writes(), 0);
    }
}
#[test]
fn bool_write_response_is_unknown_and_never_accepted_as_one() {
    let mut f = Fixture::new();
    f.ports.failure = Some("bool-response");
    assert!(f.execute("restart", false).is_err());
    f.ports.failure = None;
    assert!(f.execute("resume", false).is_err());
    f.execute("reconcile", false).unwrap();
    assert_eq!(f.writes(), 1);
}
#[test]
fn cancelled_operation_retains_intent_and_cannot_publish() {
    let mut f = Fixture::new();
    f.ports.failure = Some("after");
    assert!(f.execute("restart", false).is_err());
    let intent = f.output(1).join("owner/intent.json");
    let original = std::fs::read(&intent).unwrap();
    f.ports.guard_failure = true;
    assert!(f.execute("reconcile", false).is_err());
    assert_eq!(std::fs::read(intent).unwrap(), original);
    assert!(!f.output(1).join("owner/confirmed.json").exists());
    assert_eq!(f.writes(), 1);
}
