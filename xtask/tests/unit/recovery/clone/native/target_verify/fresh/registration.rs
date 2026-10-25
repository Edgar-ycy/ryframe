use crate::recovery::clone::native::{
    json as document,
    resources::common,
    target_verify::fresh::registration::{Registration, Workspace},
};
use crate::target_verify_fixture::Fixture;
use serde_json::Value;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

#[test]
fn registered_window_holds_the_real_run_owner_and_rejects_input_drift_without_replay() {
    let fixture = Fixture::for_status();
    let registration = Registration::load(&fixture.backend, &fixture.local).unwrap();
    let registered = document::read(&fixture.local.join("registration.json")).unwrap();
    let storage = PathBuf::from(registered["storage_run"]["path"].as_str().unwrap());
    let before = common::binding(&storage.join("state.json")).unwrap();
    let value = registration
        .execute(|active| {
            assert_eq!(active, Some(storage.as_path()));
            let owner = document::read(&storage.join("run.lock/owner.json"))?;
            assert_eq!(owner["identity"]["pid"], std::process::id());
            assert_eq!(common::binding(&storage.join("state.json"))?, before);
            Ok(json!({"remote_writes":0}))
        })
        .unwrap();
    assert_eq!(value["remote_writes"], 0);
    assert!(!storage.join("run.lock").exists());
    let path = fixture.local.join("registration.json");
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
    assert!(!storage.join("run.lock").exists());
    assert_eq!(
        common::binding(&storage.join("state.json")).unwrap(),
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
    let registered = document::read(&fixture.local.join("registration.json")).unwrap();
    let storage = PathBuf::from(registered["storage_run"]["path"].as_str().unwrap());
    fs::create_dir(storage.join("run.lock")).unwrap();
    fs::write(storage.join("run.lock/foreign.bin"), b"unowned").unwrap();
    let mut called = false;
    let result = registration.execute::<Value>(|_| {
        called = true;
        Ok(Value::Null)
    });
    assert!(result.is_err());
    assert!(!called);
    assert_eq!(
        fs::read(storage.join("run.lock/foreign.bin")).unwrap(),
        b"unowned"
    );
}
