use super::*;
use crate::recovery::clone::native::{
    copy_ledger::session::tests::{ARTIFACT, Fixture},
    copy_owner::Role,
};

#[test]
fn database_dispatch_failure_keeps_original_intent_and_publishes_unaccepted_receipt() {
    let fixture = Fixture::new();
    let step = "database-control";
    let before = json!({"kind":"database","resource_sha256":"a".repeat(64),
        "schema_sha256":"b".repeat(64),"tables_sha256":"c".repeat(64),"preserved_sha256":"d".repeat(64)});
    let mut after = before.clone();
    after["tables_sha256"] = json!("e".repeat(64));
    let mut binding = None;
    let dispatch_error = "模拟数据库发送后未收到回应";
    let mut dispatches = 0;
    let transfer = fixture.run(Mode::New, |session, owner| {
        let (backend, _, _) = owner.ledger_binding();
        let copy = owner
            .lock_path(Role::Ledger)
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        binding = Some((backend.to_owned(), copy.to_owned()));
        session.intent(step, &before, &after, ARTIFACT)?;
        session.write_once(step, &before, &after, ARTIFACT, |check| {
            check()?;
            dispatches += 1;
            Err(dispatch_error.into())
        })
    });
    let (backend, copy) = binding.unwrap();
    assert_eq!(dispatches, 1);
    assert!(!copy.join("ledger/clone.lock").exists());
    assert!(!backend.join(".local-tests/target/initialize.lock").exists());
    let attempt = Attempt::new(Mode::New, &copy);
    let error = attempt
        .failure(&backend, &copy, transfer, "transfer")
        .unwrap_err();
    assert_eq!(error.to_string(), dispatch_error);
    let receipt = copy.join("failure.json");
    let first = std::fs::read(&receipt).unwrap();
    let saved = document::read(&receipt).unwrap();
    assert_eq!(saved["kind"], "devex-copy-failure");
    assert_eq!(saved["phase"], "transfer");
    assert_eq!(saved["status"], "needs_reconciliation");
    for field in [
        "automatic_retry",
        "automatic_resource_cleanup",
        "target_ready",
        "restore_success",
    ] {
        assert_eq!(saved[field], false);
    }
    assert!(!copy.join("result.json").exists());
    assert!(
        fixture
            .run(Mode::Resume, |_, _| panic!("未知数据库步骤不允许续跑"))
            .is_err()
    );
    fixture
        .run(Mode::Reconcile, |session, _| {
            let steps = session.steps()?;
            assert_eq!(steps.len(), 1);
            assert_eq!(steps[step]["phase"], "unknown");
            assert_eq!(steps[step]["before"], before);
            assert_eq!(steps[step]["after"], after);
            assert_eq!(steps[step]["artifact_sha256"], ARTIFACT);
            Ok(())
        })
        .unwrap();
    assert_eq!(std::fs::read(receipt).unwrap(), first);
}
