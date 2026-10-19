use std::fs;

use crate::recovery::clone::native::{json as json_io, target_verify};

#[path = "target_verify_fixture.rs"]
mod fixture;

use fixture::{Fixture, database::FakeTransport, snapshot};

#[test]
fn target_verify_success_publishes_create_only_evidence() {
    let fixture = Fixture::new();
    let before = snapshot(&fixture.target);
    let run_before = snapshot(&fixture.run);
    let transport = FakeTransport::new(&fixture, false, false);
    let authority = fixture.authority();
    let storage = fixture.storage_observer();
    let result = target_verify::execute_with(
        &fixture.backend,
        &fixture.stage(),
        &transport,
        &storage,
        &authority,
    )
    .unwrap();
    assert_eq!(result["status"], "fresh_target_reverified");
    assert_eq!(result["remote_writes"], 0);
    for field in ["target_ready", "clone_verified", "restore_qualified"] {
        assert_eq!(result[field], false);
    }
    assert_eq!(
        result["initialized"],
        fixture::bound(&fixture.target.join("initialized.json"))
    );
    assert_eq!(snapshot(&fixture.target), before);
    let run_after = snapshot(&fixture.run);
    for (name, digest) in run_before {
        assert_eq!(run_after.get(&name), Some(&digest));
    }
    assert!(fixture.run.join("v0001/verify.json").is_file());
    assert!(!fixture.run.join("v0001/failure.json").exists());
    assert_eq!(transport.inventories.get(), 8);
    assert_eq!(transport.identities.get(), 16);
    assert_eq!(transport.ownerships.get(), 24);
    assert_eq!(authority.calls.get(), 2);
    storage.assert_complete();
    assert!(!fixture.target.join("initialize.lock").exists());
    assert!(
        target_verify::execute_with(
            &fixture.backend,
            &fixture.stage(),
            &transport,
            &storage,
            &authority
        )
        .is_err()
    );
    assert_eq!(snapshot(&fixture.target), before);
}

#[test]
fn target_verify_preflight_is_read_only() {
    let fixture = Fixture::new();
    let before = snapshot(&fixture.backend);
    let authority = fixture.authority();
    target_verify::preflight_with(&fixture.backend, &fixture.stage(), &authority).unwrap();
    assert_eq!(snapshot(&fixture.backend), before);
    assert!(!fixture.run.join("v0001").exists());
    assert_eq!(authority.calls.get(), 1);
}

#[test]
fn target_verify_rejects_inventory_drift_and_external_failure() {
    for (drift, fail) in [(true, false), (false, true)] {
        let fixture = Fixture::new();
        let before = snapshot(&fixture.target);
        let transport = FakeTransport::new(&fixture, drift, fail);
        let error = target_verify::execute_with(
            &fixture.backend,
            &fixture.stage(),
            &transport,
            &fixture.storage_observer(),
            &fixture.authority(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("库存") || error.contains("失败"), "{error}");
        assert!(fixture.run.join("v0001/failure.json").is_file());
        assert!(!fixture.run.join("v0001/verify.json").exists());
        assert_eq!(snapshot(&fixture.target), before);
        assert!(!fixture.target.join("initialize.lock").exists());
        let failure = json_io::read(&fixture.run.join("v0001/failure.json")).unwrap();
        assert_eq!(failure["automatic_retry"], false);
        assert_eq!(failure["automatic_resource_cleanup"], false);
        assert_eq!(failure["clone_verified"], false);
        assert_eq!(transport.inventories.get(), if fail { 1 } else { 8 });
    }
}

#[test]
fn target_verify_respects_target_generation_guard() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.target.join("initialize.lock")).unwrap();
    let before = snapshot(&fixture.target);
    let transport = FakeTransport::new(&fixture, false, false);
    let error = target_verify::execute_with(
        &fixture.backend,
        &fixture.stage(),
        &transport,
        &fixture.storage_observer(),
        &fixture.authority(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("初始化锁"), "{error}");
    assert_eq!(transport.inventories.get(), 0);
    assert!(!fixture.run.join("v0001").exists());
    assert_eq!(snapshot(&fixture.target), before);
    assert!(fixture.target.join("initialized.json").is_file());
}
