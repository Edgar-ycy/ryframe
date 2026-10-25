use super::*;
use crate::target_verify_fixture::Fixture;
use serde_json::json;
use std::fs;

#[test]
fn registered_window_holds_the_real_run_owner_and_rejects_input_drift_without_replay() {
    let fixture = Fixture::for_status();
    let registration = Registration::load(&fixture.backend, &fixture.local).unwrap();
    let before = common::binding(&registration.storage.join("state.json")).unwrap();
    let value = registration
        .execute(|active| {
            assert_eq!(active, Some(registration.storage.as_path()));
            let owner = document::read(&registration.storage.join("run.lock/owner.json"))?;
            assert_eq!(owner["identity"]["pid"], std::process::id());
            assert_eq!(
                common::binding(&registration.storage.join("state.json"))?,
                before
            );
            Ok(json!({"remote_writes":0}))
        })
        .unwrap();
    assert_eq!(value["remote_writes"], 0);
    assert!(!registration.storage.join("run.lock").exists());
    let path = registration.workspace.join("registration.json");
    let raw = fs::read(&path).unwrap();
    let error = registration
        .execute::<Value>(|_| {
            fs::write(&path, b"{}")?;
            Err("original-observation-failure".into())
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("original-observation-failure"));
    assert!(error.contains("登记"));
    fs::write(&path, raw).unwrap();
    assert!(!registration.storage.join("run.lock").exists());
    assert_eq!(
        common::binding(&registration.storage.join("state.json")).unwrap(),
        before
    );
}

#[test]
fn workspace_guard_refuses_replacement_and_never_cleans_another_owner_lock() {
    let fixture = Fixture::for_status();
    let workspace = fixture.local.join("prepare-workspace");
    fs::create_dir(&workspace).unwrap();
    let guard = Workspace::claim(&fixture.backend, &workspace).unwrap();
    assert!(Workspace::claim(&fixture.backend, &workspace).is_err());
    guard.finish(Ok(())).unwrap();
    let registration = Registration::load(&fixture.backend, &fixture.local).unwrap();
    fs::create_dir(registration.storage.join("run.lock")).unwrap();
    fs::write(
        registration.storage.join("run.lock/foreign.bin"),
        b"unowned",
    )
    .unwrap();
    let mut called = false;
    let result = registration.execute::<Value>(|_| {
        called = true;
        Ok(Value::Null)
    });
    assert!(result.is_err());
    assert!(!called);
    assert_eq!(
        fs::read(registration.storage.join("run.lock/foreign.bin")).unwrap(),
        b"unowned"
    );
}
